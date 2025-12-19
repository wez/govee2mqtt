FROM python:3.11-slim

WORKDIR /app

COPY python/pyproject.toml /app/pyproject.toml
COPY python/README.md /app/README.md
COPY python/src /app/src

RUN pip install --no-cache-dir .

ENV PYTHONUNBUFFERED=1

CMD ["govee2mqtt-v2"]
