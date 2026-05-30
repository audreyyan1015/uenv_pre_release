.PHONY: all proto build build-server build-worker build-hub build-adapter-core test test-server test-worker test-hub test-adapter-core clean

all: proto build

# ─── Protobuf ────────────────────────────────────────────────
PROTO_SERVER = uenv-server/proto/server.proto
PROTO_WORKER = uenv-worker/proto/worker.proto
PROTO_HUB    = uenv-hub/proto/hub.proto
PROTO_ADAPTER_CORE = uenv-bridge/proto/adapter_core.proto
PYTHON ?= python3

proto: proto-server proto-worker proto-hub proto-bridge proto-adapter-core

proto-server:
	protoc -I=uenv-server/proto \
		$(PROTO_SERVER) \
		--rust_out=uenv-server/src/gen \
		--tonic_out=uenv-server/src/gen

proto-worker:
	protoc -I=uenv-worker/proto -I=uenv-server/proto \
		$(PROTO_WORKER) \
		--rust_out=uenv-worker/src/gen \
		--tonic_out=uenv-worker/src/gen

proto-hub:
	protoc -I=uenv-hub/proto -I=uenv-server/proto \
		$(PROTO_HUB) \
		--rust_out=uenv-hub/src/gen \
		--tonic_out=uenv-hub/src/gen

proto-bridge:
	protoc -I=uenv-server/proto \
		$(PROTO_SERVER) \
		--python_out=uenv-bridge/src/gen \
		--grpc_python_out=uenv-bridge/src/gen

proto-adapter-core:
	mkdir -p uenv-bridge/src/uenv/bridge/gen
	cd uenv-bridge && $(PYTHON) -m grpc_tools.protoc \
		-I=proto \
		proto/adapter_core.proto \
		--python_out=src/uenv/bridge/gen \
		--grpc_python_out=src/uenv/bridge/gen

# ─── Build (每个 part 独立编译，target 在各自目录内) ──────────
build: build-server build-worker build-hub build-adapter-core

build-server:
	cd uenv-server && cargo build

build-worker:
	cd uenv-worker && cargo build

build-hub:
	cd uenv-hub && cargo build

build-adapter-core:
	cd uenv-bridge/core && cargo build

# ─── Test ─────────────────────────────────────────────────────
test: test-server test-worker test-hub test-adapter-core

test-server:
	cd uenv-server && cargo test

test-worker:
	cd uenv-worker && cargo test

test-hub:
	cd uenv-hub && cargo test

test-adapter-core:
	cd uenv-bridge/core && cargo test

# ─── Clean ────────────────────────────────────────────────────
clean:
	cd uenv-server && cargo clean
	cd uenv-worker && cargo clean
	cd uenv-hub && cargo clean
	cd uenv-bridge/core && cargo clean
	rm -rf uenv-server/src/gen uenv-worker/src/gen uenv-hub/src/gen uenv-bridge/src/gen uenv-bridge/src/uenv/bridge/gen
