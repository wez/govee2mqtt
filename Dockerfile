####################################################################################################
## Builder
####################################################################################################
FROM --platform=$BUILDPLATFORM alpine:latest AS builder
ARG TARGETPLATFORM

# Rust und benötigte Tools installieren
RUN apk add --no-cache build-base openssl-dev curl
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# musl Target hinzufügen
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /work
COPY . .

# Für musl kompilieren
RUN cargo build --release --target x86_64-unknown-linux-musl

# User wie gehabt anlegen
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "1000" \
    "govee"

WORKDIR /data

####################################################################################################
## Final image
####################################################################################################
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group

WORKDIR /app

# Das statisch gelinkte musl-Binary kopieren!
COPY --from=builder /work/target/x86_64-unknown-linux-musl/release/govee /app/govee
COPY AmazonRootCA1.pem /app
COPY --from=builder --chown=govee:govee /data /data
COPY assets /app/assets

USER govee:govee
LABEL org.opencontainers.image.source="https://github.com/wez/govee2mqtt"
ENV \
  RUST_BACKTRACE=full \
  PATH=/app:$PATH \
  XDG_CACHE_HOME=/data

VOLUME /data

CMD ["/app/govee", \
  "serve", \
  "--govee-iot-key=/data/iot.key", \
  "--govee-iot-cert=/data/iot.cert", \
  "--amazon-root-ca=/app/AmazonRootCA1.pem"]
