use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use reqwest::blocking::{Client, multipart};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const DEFAULT_ZAPCAP_URL: &str = "https://api.zapcap.ai";

#[derive(Debug, Parser)]
#[command(
    name = "captions",
    version,
    about = "Caption rendering CLI",
    after_help = "Examples:\n  captions zapcap templates --pretty\n  captions zapcap render --video input.mp4 --template-id <id> --out captioned.mp4\n  captions zapcap render --video-url https://example.com/input.mp4 --template-id <id> --out captioned.mp4"
)]
struct Cli {
    #[arg(long, global = true, default_value = DEFAULT_ZAPCAP_URL)]
    zapcap_url: String,
    #[arg(long, global = true)]
    pretty: bool,
    #[arg(long, global = true)]
    raw: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "ZapCap caption renderer")]
    Zapcap {
        #[command(subcommand)]
        command: ZapcapCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ZapcapCommand {
    #[command(about = "List ZapCap templates")]
    Templates,
    #[command(about = "Upload/import a video, create a caption task, optionally wait")]
    Render(RenderArgs),
    #[command(about = "ZapCap task operations")]
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
}

#[derive(Debug, Args)]
struct RenderArgs {
    #[arg(
        long,
        conflicts_with = "video_url",
        help = "Local video path to upload"
    )]
    video: Option<PathBuf>,
    #[arg(long, conflicts_with = "video", help = "Public video URL to import")]
    video_url: Option<String>,
    #[arg(long)]
    template_id: String,
    #[arg(long, default_value = "en")]
    language: String,
    #[arg(long, help = "Create task but do not approve automatically")]
    manual_approve: bool,
    #[arg(long, help = "Submit only and print IDs")]
    no_wait: bool,
    #[arg(long, help = "Write rendered MP4 here")]
    out: Option<PathBuf>,
    #[arg(long, default_value_t = 5)]
    poll_interval_secs: u64,
    #[arg(long, default_value_t = 1200)]
    max_wait_secs: u64,
}

#[derive(Debug, Subcommand)]
enum TaskCommand {
    #[command(about = "Fetch task status")]
    Get(TaskArgs),
    #[command(about = "Wait for task completion")]
    Wait(TaskWaitArgs),
}

#[derive(Debug, Args)]
struct TaskArgs {
    #[arg(long)]
    video_id: String,
    #[arg(long)]
    task_id: String,
}

#[derive(Debug, Args)]
struct TaskWaitArgs {
    #[arg(long)]
    video_id: String,
    #[arg(long)]
    task_id: String,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long, default_value_t = 5)]
    poll_interval_secs: u64,
    #[arg(long, default_value_t = 1200)]
    max_wait_secs: u64,
}

struct ZapcapClient {
    http: Client,
    api_key: String,
    base_url: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Zapcap { command } => {
            let api = ZapcapClient::new(cli.zapcap_url)?;
            match command {
                ZapcapCommand::Templates => print_json(&api.templates()?, cli.pretty),
                ZapcapCommand::Render(args) => handle_render(&api, args, cli.pretty, cli.raw),
                ZapcapCommand::Task { command } => match command {
                    TaskCommand::Get(args) => {
                        print_json(&api.task_status(&args.video_id, &args.task_id)?, cli.pretty)
                    }
                    TaskCommand::Wait(args) => {
                        let result = api.wait_task(
                            &args.video_id,
                            &args.task_id,
                            args.max_wait_secs,
                            args.poll_interval_secs,
                        )?;
                        if let Some(out) = args.out {
                            let url = extract_download_url(&result)
                                .context("task missing downloadUrl")?;
                            api.download_to(&url, &out)?;
                            print_json(
                                &json!({"video_id": args.video_id, "task_id": args.task_id, "out": out, "download_url": url}),
                                cli.pretty,
                            )
                        } else {
                            print_json(&result, cli.pretty)
                        }
                    }
                },
            }
        }
    }
}

impl ZapcapClient {
    fn new(base_url: String) -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .user_agent(concat!("captions-cli/", env!("CARGO_PKG_VERSION")))
                .build()?,
            api_key: read_secret("ZAPCAP_API_KEY")?,
            base_url,
        })
    }

    fn templates(&self) -> Result<Value> {
        let url = format!("{}/templates", self.base_url.trim_end_matches('/'));
        decode(
            self.http
                .get(url)
                .header("x-api-key", &self.api_key)
                .send()?,
        )
    }

    fn upload_video(&self, path: &Path) -> Result<String> {
        let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("video.mp4")
            .to_string();
        let part = multipart::Part::reader(file)
            .file_name(name)
            .mime_str(mime_for_path(path))?;
        let form = multipart::Form::new().part("file", part);
        let url = format!("{}/videos", self.base_url.trim_end_matches('/'));
        let v = decode(
            self.http
                .post(url)
                .header("x-api-key", &self.api_key)
                .multipart(form)
                .send()?,
        )?;
        extract_id(&v).context("upload response missing id")
    }

    fn import_video_url(&self, video_url: &str) -> Result<String> {
        let url = format!("{}/videos/url", self.base_url.trim_end_matches('/'));
        let v = decode(
            self.http
                .post(url)
                .header("x-api-key", &self.api_key)
                .json(&json!({"url": video_url}))
                .send()?,
        )?;
        extract_id(&v).context("URL import response missing id")
    }

    fn create_task(
        &self,
        video_id: &str,
        template_id: &str,
        language: &str,
        auto_approve: bool,
    ) -> Result<String> {
        let url = format!(
            "{}/videos/{}/task",
            self.base_url.trim_end_matches('/'),
            video_id
        );
        let body =
            json!({"templateId": template_id, "autoApprove": auto_approve, "language": language});
        let v = decode(
            self.http
                .post(url)
                .header("x-api-key", &self.api_key)
                .json(&body)
                .send()?,
        )?;
        v.get("taskId")
            .or_else(|| v.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .context("task response missing taskId")
    }

    fn task_status(&self, video_id: &str, task_id: &str) -> Result<Value> {
        let url = format!(
            "{}/videos/{}/task/{}",
            self.base_url.trim_end_matches('/'),
            video_id,
            task_id
        );
        decode(
            self.http
                .get(url)
                .header("x-api-key", &self.api_key)
                .send()?,
        )
    }

    fn wait_task(
        &self,
        video_id: &str,
        task_id: &str,
        max_wait_secs: u64,
        poll_interval_secs: u64,
    ) -> Result<Value> {
        let start = Instant::now();
        loop {
            let status = self.task_status(video_id, task_id)?;
            match normalize_status(&status).as_deref() {
                Some("completed") | Some("complete") | Some("succeeded") | Some("success") => {
                    return Ok(status);
                }
                Some("failed") | Some("error") | Some("errored") | Some("canceled")
                | Some("cancelled") => {
                    bail!("ZapCap task failed: {}", status)
                }
                _ => {
                    if start.elapsed() > Duration::from_secs(max_wait_secs) {
                        bail!("timeout waiting for task {task_id}");
                    }
                    thread::sleep(Duration::from_secs(poll_interval_secs.max(1)));
                }
            }
        }
    }

    fn download_to(&self, url: &str, out: &Path) -> Result<()> {
        let res = self.http.get(url).send()?;
        let status = res.status();
        let bytes = res.bytes()?;
        if !status.is_success() {
            bail!("download failed: http {status}");
        }
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::write(out, bytes).with_context(|| format!("write {}", out.display()))
    }
}

fn handle_render(api: &ZapcapClient, args: RenderArgs, pretty: bool, raw: bool) -> Result<()> {
    let video_id = match (args.video, args.video_url) {
        (Some(path), None) => api.upload_video(&path)?,
        (None, Some(url)) => api.import_video_url(&url)?,
        _ => bail!("provide exactly one of --video or --video-url"),
    };
    let task_id = api.create_task(
        &video_id,
        &args.template_id,
        &args.language,
        !args.manual_approve,
    )?;
    if args.no_wait {
        return print_json(&json!({"video_id": video_id, "task_id": task_id}), pretty);
    }
    let out = args
        .out
        .context("--out is required unless --no-wait is set")?;
    let result = api.wait_task(
        &video_id,
        &task_id,
        args.max_wait_secs,
        args.poll_interval_secs,
    )?;
    let url = extract_download_url(&result).context("task missing downloadUrl")?;
    api.download_to(&url, &out)?;
    if raw {
        print_json(&result, pretty)
    } else {
        print_json(
            &json!({"video_id": video_id, "task_id": task_id, "out": out, "download_url": url}),
            pretty,
        )
    }
}

fn decode(res: reqwest::blocking::Response) -> Result<Value> {
    let status = res.status();
    let text = res.text()?;
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({"raw": text}));
    if !status.is_success() {
        bail!("http {status}: {parsed}");
    }
    Ok(parsed)
}

fn read_secret(name: &str) -> Result<String> {
    if let Ok(v) = env::var(name) {
        if !v.trim().is_empty() {
            return Ok(v.trim().to_string());
        }
    }
    let p = format!("/run/secrets/{name}");
    if let Ok(v) = fs::read_to_string(&p) {
        if !v.trim().is_empty() {
            return Ok(v.trim().to_string());
        }
    }
    bail!("{name} missing; set ${name} or /run/secrets/{name}")
}

fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

fn extract_id(v: &Value) -> Option<String> {
    v.get("id")
        .or_else(|| v.get("videoId"))
        .or_else(|| v.get("video_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn extract_download_url(v: &Value) -> Option<String> {
    v.get("downloadUrl")
        .or_else(|| v.get("download_url"))
        .or_else(|| v.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn normalize_status(v: &Value) -> Option<String> {
    v.get("status")
        .and_then(Value::as_str)
        .map(|s| s.to_ascii_lowercase())
}

fn print_json(value: &Value, pretty: bool) -> Result<()> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", serde_json::to_string(value)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_download_url() {
        let v = json!({"status":"completed", "downloadUrl":"https://example.com/out.mp4"});
        assert_eq!(
            extract_download_url(&v).as_deref(),
            Some("https://example.com/out.mp4")
        );
    }

    #[test]
    fn normalizes_status() {
        let v = json!({"status":"COMPLETED"});
        assert_eq!(normalize_status(&v).as_deref(), Some("completed"));
    }
}
