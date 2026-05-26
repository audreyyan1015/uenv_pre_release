# uenv-mock-scheduler

Mock ControlPlane for Worker Pool MVP.

It accepts worker `RegisterWorker` / `WorkerHeartbeat` / `ReportResult`, and actively dispatches fixture-backed `DispatchEpisode` requests to registered workers.

## CLI

```bash
uenv-mock-scheduler serve --config config/uenv-mock-scheduler.yaml
uenv-mock-scheduler version
```

## Logging

- Default log file: `/var/log/uenv/mock-scheduler.log`
- CLI override: `--log-file ./mock-scheduler.log`
- Env override: `UENV_LOG_FILE=./mock-scheduler.log`

```bash
tail -f /var/log/uenv/mock-scheduler.log
```

## Fault Injection (M1.5)

- `UENV_MOCK_DISPATCH_DELAY_MS`: delay before each active dispatch.
- `UENV_MOCK_DROP_HEARTBEAT_N`: drop ACK for first N heartbeat messages.
- `UENV_MOCK_DUPLICATE_DISPATCH`: duplicate each dispatch when set to `1` or `true`.
- `UENV_MOCK_SERVER_EPOCH`: inject fixed `server_epoch` value in control-plane responses.

Mapping to M1.7 scenarios:

- `duplicate_dispatch` -> `UENV_MOCK_DUPLICATE_DISPATCH=1`
- `heartbeat_timeout` -> `UENV_MOCK_DROP_HEARTBEAT_N=<N>`
- dispatch delay / timing perturbation -> `UENV_MOCK_DISPATCH_DELAY_MS=<ms>`
- `stale_worker_id` / `server_epoch` change -> `UENV_MOCK_SERVER_EPOCH=<epoch>`

## Manual Probe (grpcurl)

```bash
# register worker
grpcurl -plaintext -d '{"worker_id":"w1","supported_env_types":["gsm8k"],"endpoint":"127.0.0.1:50052","max_concurrent":1}' \
  127.0.0.1:50051 uenv.scheduler.v1.ControlPlaneService/RegisterWorker

# list workers
grpcurl -plaintext -d '{"env_types":["gsm8k"]}' \
  127.0.0.1:50051 uenv.scheduler.v1.ControlPlaneService/ListWorkers
```
