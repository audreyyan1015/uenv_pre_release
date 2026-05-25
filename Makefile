.PHONY: all proto build build-server build-worker build-mock-scheduler build-hub test test-server test-worker test-mock-scheduler test-hub clean

all: proto build

# ─── Protobuf ────────────────────────────────────────────────
PROTO_ROOT   = proto
PROTO_SERVER = uenv-server/proto/server.proto
PROTO_WORKER = uenv-worker/proto/worker_service.proto
PROTO_HUB    = uenv-hub/proto/hub.proto
PROTO_SCHED  = $(PROTO_ROOT)/uenv/v1/scheduler.proto
PROTO_PLUGIN = plugin_proto/uenv/plugin/v1/plugin.proto

proto: proto-server proto-worker proto-mock-scheduler proto-hub proto-bridge proto-plugin

proto-server:
	protoc -I=$(PROTO_ROOT) -I=uenv-server/proto \
		$(PROTO_SERVER) \
		$(PROTO_ROOT)/uenv/v1/episode.proto \
		$(PROTO_ROOT)/uenv/v1/common.proto \
		$(PROTO_ROOT)/uenv/v1/wal.proto \
		--prost_out=uenv-server/src/gen \
		--tonic_out=uenv-server/src/gen

proto-worker:
	protoc -I=$(PROTO_ROOT) -I=uenv-worker/proto \
		$(PROTO_WORKER) \
		$(PROTO_ROOT)/uenv/v1/episode.proto \
		$(PROTO_ROOT)/uenv/v1/common.proto \
		$(PROTO_ROOT)/uenv/v1/wal.proto \
		--prost_out=uenv-worker/src/gen \
		--tonic_out=uenv-worker/src/gen

proto-mock-scheduler:
	protoc -I=$(PROTO_ROOT) \
		$(PROTO_SCHED) \
		$(PROTO_ROOT)/uenv/v1/episode.proto \
		$(PROTO_ROOT)/uenv/v1/common.proto \
		$(PROTO_ROOT)/uenv/v1/wal.proto \
		--prost_out=uenv-mock-scheduler/src/gen \
		--tonic_out=uenv-mock-scheduler/src/gen

proto-hub:
	protoc -I=$(PROTO_ROOT) -I=uenv-hub/proto \
		$(PROTO_HUB) \
		$(PROTO_ROOT)/uenv/v1/episode.proto \
		$(PROTO_ROOT)/uenv/v1/common.proto \
		--prost_out=uenv-hub/src/gen \
		--tonic_out=uenv-hub/src/gen

proto-bridge:
	protoc -I=$(PROTO_ROOT) -I=uenv-server/proto \
		$(PROTO_SERVER) \
		$(PROTO_ROOT)/uenv/v1/episode.proto \
		$(PROTO_ROOT)/uenv/v1/common.proto \
		--python_out=uenv-bridge/src/gen \
		--grpc_python_out=uenv-bridge/src/gen

proto-plugin:
	protoc -I=plugin_proto \
		$(PROTO_PLUGIN) \
		--prost_out=uenv-worker/src/gen \
		--tonic_out=uenv-worker/src/gen

# ─── Build ───────────────────────────────────────────────────
build: build-server build-worker build-mock-scheduler build-hub

build-server:
	cd uenv-server && cargo build

build-worker:
	cd uenv-worker && cargo build

build-mock-scheduler:
	cd uenv-mock-scheduler && cargo build

build-hub:
	cd uenv-hub && cargo build

# ─── Test ─────────────────────────────────────────────────────
test: test-server test-worker test-mock-scheduler test-hub

test-server:
	cd uenv-server && cargo test

test-worker:
	cd uenv-worker && cargo test

test-mock-scheduler:
	cd uenv-mock-scheduler && cargo test

test-hub:
	cd uenv-hub && cargo test

# ─── Clean ────────────────────────────────────────────────────
clean:
	cd uenv-server && cargo clean
	cd uenv-worker && cargo clean
	cd uenv-mock-scheduler && cargo clean
	cd uenv-hub && cargo clean
	rm -rf uenv-server/src/gen uenv-worker/src/gen uenv-mock-scheduler/src/gen uenv-hub/src/gen uenv-bridge/src/gen
