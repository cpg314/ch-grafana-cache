FROM debian:bookworm-slim

LABEL org.opencontainers.image.source=https://github.com/cpg314/ch-grafana-cache
LABEL org.opencontainers.image.licenses=MIT

COPY target-cross/x86_64-unknown-linux-gnu/release/ch-grafana-cache /usr/bin/ch-grafana-cache

CMD ["/usr/bin/ch-grafana-cache"]
