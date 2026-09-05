FROM dockerproxy.net/library/rust:1.96-bookworm AS build
WORKDIR /src
COPY deploy/kubernetes/uenv-swe/tooling/protoc /usr/local/bin/protoc
COPY deploy/kubernetes/uenv-swe/tooling/linux/include /usr/local/include
COPY . .
COPY vendor /src/vendor
RUN printf '%s\n' '[source.crates-io]' 'replace-with = "vendored-sources"' '' '[source.vendored-sources]' 'directory = "/src/vendor"' > /src/.cargo-config.toml
ARG HTTP_PROXY
ARG HTTPS_PROXY
ARG ALL_PROXY
ENV HTTP_PROXY=${HTTP_PROXY} HTTPS_PROXY=${HTTPS_PROXY} ALL_PROXY=${ALL_PROXY}
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
ENV CARGO_REGISTRIES_CRATES_IO_INDEX=https://rsproxy.cn/index/
ENV RUSTUP_TOOLCHAIN=1.96.1
RUN . /usr/local/cargo/env && CARGO_HOME=/usr/local/cargo PROTOC=/usr/local/bin/protoc cargo build --offline --config /src/.cargo-config.toml --release -p uenv-worker --bin uenv-worker
RUN install -Dm755 target/release/uenv-worker /usr/local/bin/uenv-worker
COPY config/uenv-worker.yaml /etc/uenv/worker.yaml
EXPOSE 28888 28097 19090
ENTRYPOINT ["/usr/local/bin/uenv-worker"]
CMD ["--config", "/etc/uenv/worker.yaml", "serve"]
