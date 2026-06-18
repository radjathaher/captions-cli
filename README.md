# captions-cli

Caption-rendering CLI. v0.1 wraps ZapCap.

## Auth

```sh
export ZAPCAP_API_KEY="..."
```

The CLI also falls back to `/run/secrets/ZAPCAP_API_KEY`.

## Usage

```sh
captions zapcap templates --pretty

captions zapcap render \
  --video input.mp4 \
  --template-id <template-id> \
  --language en \
  --out captioned.mp4

captions zapcap render --video-url https://example.com/input.mp4 --template-id <template-id> --out captioned.mp4
captions zapcap task wait --video-id <video-id> --task-id <task-id> --out captioned.mp4
```

ZapCap requires Pro+ API access and API credits.
