ARG BUILD_FROM
FROM ghcr.io/wez/govee2mqtt:latest as govee2mqtt
FROM $BUILD_FROM
COPY run.sh /run.sh
COPY --from=govee2mqtt /app/govee /app/assets /app/AmazonRootCA1.pem /app/
CMD [ "/run.sh" ]
