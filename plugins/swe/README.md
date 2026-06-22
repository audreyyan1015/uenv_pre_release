# `plugins/swe` — SWE-bench OpenEnv plugin

OpenEnv-style environment + evaluator for SWE-bench (plan §4.2 / §5.3.4 / §7).
Drives the Worker **External Runtime Gateway** (L4 HTTP), so it shares the same
sandbox (L2 `SweInstancePool` → L1 Docker backend) and grader (`swebench` /
`swebench_pro`) as native `DispatchEpisode(env_type=swe)` and the OpenHands
integration — one execution contract, no divergence.

## Contract

```
reset()        -> SweObservation   # provision sandbox, return problem_statement
step(action)   -> StepResult       # exec | read | write | apply_patch (bash -lc)
evaluate()     -> EvalResult        # apply test_patch + run tests + grade (reward)
close()                              # release sandbox
```

| File | Role |
|------|------|
| `environment.py` | `SweEnvironment` (OpenEnv reset/step/evaluate/close). |
| `command_policy.py` | `CommandPolicy` mirror of Rust `swe::command_policy` (`RestrictedShell`/`FullShell`; `deny_patterns` MVP-only). |
| `evaluator/swe_evaluator.py` | `SweEvaluator` (gateway `submit`) + `parse_pytest_report` offline fallback. |
| `server/app.py` | Minimal stdlib HTTP server exposing the environment (OpenEnv endpoint). |
| `tests/test_environment.py` | Offline unit tests (policy + parser, no network). |

## Usage

```python
from plugins.swe import SweEnvironment, SweAction

with SweEnvironment("scikit-learn__scikit-learn-14141",
                    gateway_url="127.0.0.1:48999",
                    benchmark_variant="verified") as env:
    obs = env.reset()
    env.step(SweAction(type="apply_patch", content=gold_patch))
    result = env.evaluate()     # -> reward 1.0 if resolved
```

As an HTTP env server:

```bash
python3 -m plugins.swe.server.app --listen 127.0.0.1:8900 --gateway 127.0.0.1:48999
```

Offline tests:

```bash
python3 -m pytest plugins/swe/tests -q
```

> `deny_patterns` is an MVP-only substring helper enforced client-side before
> forwarding; the real boundary is the container capability profile (plan §1.4).
