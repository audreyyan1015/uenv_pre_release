# Multi-stage build for uenv-hub-server.
# Build context should be the `uenv-hub/` workspace root.

FROM rust:1-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p uenv-hub-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 uenv \
    && mkdir -p /var/lib/uenv-hub \
    && chown uenv:uenv /var/lib/uenv-hub

COPY --from=builder /build/target/release/uenv-hub-server /usr/local/bin/uenv-hub-server

USER uenv
ENV UENV_HUB_SERVER__HOST=0.0.0.0 \
    UENV_HUB_SERVER__PORT=8080 \
    UENV_HUB_DATABASE__URL=sqlite:///var/lib/uenv-hub/hub.db
VOLUME ["/var/lib/uenv-hub"]
EXPOSE 8080

# Container-level health check hits the public /healthz endpoint.
HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD ["/usr/local/bin/uenv-hub-server", "--help"]

ENTRYPOINT ["/usr/local/bin/uenv-hub-server"]
