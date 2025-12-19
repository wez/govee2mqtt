# Docker (Python v2)

This doc covers the Docker usage for the Python implementation.

## Build

```bash
docker build -f python/Dockerfile -t govee2mqtt-v2 .
```

## Run

```bash
docker run --rm --env-file .env --network host govee2mqtt-v2
```

## Compose

Use `docker-compose.python.yml`:

```bash
docker compose -f docker-compose.python.yml up
```

## Engineering Notes

- Host networking is used to match the existing repo defaults.
- Rate limits are enforced in the app; adjust `POLL_INTERVAL_SECONDS` if needed.
