ARG BUILD_FROM=ghcr.io/hassio-addons/base:14.0.0
FROM ${BUILD_FROM}

ENV LANG C.UTF-8

WORKDIR /app

COPY python/pyproject.toml /app/pyproject.toml
COPY python/README.md /app/README.md
COPY python/src /app/src

RUN pip install --no-cache-dir .

COPY addon-v2/run.sh /run.sh
RUN chmod +x /run.sh

CMD ["/run.sh"]
