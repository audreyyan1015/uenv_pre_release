# UEnv ⇄ OpenHands integration (`UEnvRuntime`)

Adapts an OpenHands agent loop onto the UEnv Worker **External Runtime Gateway**
(L4, HTTP) so OpenHands can run SWE-bench instances inside UEnv-managed sandboxes
(plan §5.3.3 / §5.6).

```
OpenHands agent  ──actions──▶  UEnvRuntime  ──HTTP──▶  Worker L4 Gateway
 (CmdRunAction,                (this package)          /runtime/v1/sessions/...
  FileWriteAction, …)                                   │
                                                        ▼
                                              L2 SweInstancePool → L1 Backend (Docker)
```

## Layout

| File | Role |
|------|------|
| `uenv_runtime/client.py` | `UEnvGatewayClient` / `UEnvSession` — dependency-free (`urllib`) HTTP client for the gateway contract. |
| `uenv_runtime/runtime.py` | `UEnvRuntime` — duck-typed adapter exposing OpenHands `run`/`read`/`write`/`run_action`. |
| `run_swebench.py` | End-to-end driver: connect → apply edits as OpenHands actions → `submit`. |
| `tests/test_client_smoke.py` | Offline adapter unit tests + opt-in live gateway check. |

## Design notes / decoupling decision (vs plan §5.3.3)

**Decision (confirmed): this integration is an independent rewrite and has ZERO
dependency on the OpenHands package — we never `import openhands`.**

This is a deliberate deviation from plan §5.3.3, which assumed `UEnvRuntime` would
**implement/subclass OpenHands' classic `Runtime`** (`run`/`read`/`write`/`copy`)
and drive evaluation via OpenHands `evaluation/benchmarks/swe_bench` (pinning the
OpenHands swebench branch, plan Q6).

Reason: the vendored OpenHands (`openhands-ai`) is the **new
`app_server`/`agent_server`/SDK architecture**, which ships **none** of the pieces
the plan targets:

- no `openhands/runtime/base.py` (classic `Runtime` ABC),
- no `openhands/events/observation*`,
- no `evaluation/benchmarks/swe_bench` driver.

(The new architecture instead exposes a *runtime-api* protocol — `/start`,
`/sessions/{id}`, `/list`, `/pause` … — in `app_server/sandbox/remote_sandbox_service.py`.)

So `UEnvRuntime`:

- **duck-types** action objects (reads `.command` / `.path` / `.content`), accepting
  classic action dataclasses or plain dicts, without binding to any OpenHands release;
- returns **OpenHands-shaped plain dicts** (same field names as
  `CmdOutputObservation` / `FileReadObservation` / `FileWriteObservation`); it does
  **not** import or construct real OpenHands observation types;
- does **not** subclass OpenHands' `Runtime`.

The `UEnvGatewayClient` is fully standalone and is the artifact validated against a
live Worker gateway offline (gold→reward=1.0 / no-gold→0). Wiring a real OpenHands
LLM agent loop additionally requires OpenHands + model access (online) and is out of
scope for the offline Worker host.

**If a true OpenHands dependency is later required** (plan §5.3.3 literal), pin an
OpenHands release that still ships the classic `Runtime` + `benchmarks/swe_bench`,
then add a thin subclass shim that delegates to `UEnvGatewayClient` — the gateway
contract here is unchanged.

## Quick start

Start a Worker with the gateway enabled (see `config/uenv-worker.swe-local.yaml`),
then:

```bash
# gold-patch replay → reward should be 1.0
python3 integrations/openhands/run_swebench.py \
    --gateway 127.0.0.1:48999 \
    --instance scikit-learn__scikit-learn-14141 \
    --instances fixtures/swe/swe_instances.json

# negative control → reward 0.0
python3 integrations/openhands/run_swebench.py ... --no-gold

# offline adapter unit tests
python3 -m pytest integrations/openhands/tests -q
```

Environment:

- `--gateway` / `UENV_GATEWAY`: gateway `host:port` (or full URL).
- `--benchmark-variant`: `verified` | `lite` | `pro` (selects the grader server-side).
- `--command-mode`: `FullShell` (default) | `RestrictedShell`.
