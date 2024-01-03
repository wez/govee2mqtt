####################################################################################################
## Builder
####################################################################################################
FROM --platform=$BUILDPLATFORM alpine:latest AS builder
ARG TARGETPLATFORM

RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "1000" \
    "govee"

WORKDIR /work
COPY docker-target/$TARGETPLATFORM/govee /work

# Creates an empty /data dir that we can use to copy and chown in the next stage
WORKDIR /data

####################################################################################################
## Final image
####################################################################################################
FROM gcr.io/distroless/cc-debian12

# Import from builder.
COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group
#COPY --from=builder /etc/ssl/certs /etc/ssl/certs

WORKDIR /app

COPY --from=builder /work/govee /app/govee
COPY AmazonRootCA1.pem /app
COPY --from=builder --chown=govee:govee /data /data
COPY assets /app/assets

USER govee:govee
LABEL org.opencontainers.image.source="https://github.com/wez/govee"
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


