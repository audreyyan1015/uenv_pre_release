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

## Design notes / version compatibility

This repo vendors the **new** OpenHands (`app_server` + `agent_server` + SDK)
architecture, which no longer ships the classic `openhands.runtime.base.Runtime`
ABC or `evaluation/benchmarks/swe_bench`. To stay decoupled from any single
OpenHands release, `UEnvRuntime`:

- **duck-types** action objects (reads `.command` / `.path` / `.content`), so it
  accepts both classic action dataclasses and plain dicts;
- returns real `openhands.events.observation.*` objects **if importable**, else
  equivalent dicts;
- does **not** subclass OpenHands' `Runtime` (which pulls in a full sandbox /
  plugin stack). It is the minimal object a `swe_bench` driver sends actions to.

The `UEnvGatewayClient` is fully standalone and is the artifact validated against
a live Worker gateway offline. Wiring a real OpenHands LLM agent loop additionally
requires OpenHands + model access (online) and is out of scope for the offline
Worker host.

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
