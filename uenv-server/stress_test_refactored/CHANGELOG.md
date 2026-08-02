# Refactor changes

- Moved frozen trace storage from `/opt/uenv-stress` to
  `/home/uenv-stress`; refactored configs and tool defaults now use the new
  paths, while `/opt` remains the home for datasets, environments, runtime
  configuration, and run artifacts.
- Added immutable Doubao/Qwen mixed-corpus preparation: four datasets use
  strict same-question 1:1 alternating pairs, while SWE-bench Pro keeps exactly
  50 Doubao trajectories.
- Replaced independent model-time simulation with recorded latency or the
  explicitly labeled Episode elapsed proxy; multi-turn waits preserve the exact
  Episode total and missing data is median-imputed only below the 5% gate.
- Added a formal `pressure` phase at exactly 10 times the frozen
  reference/stability submission rates while keeping the same corpus, selector,
  seed, and wait plan.
- Recorded intentional pressure overload separately from the fail-closed
  stability capacity gate. The current 1024x8 proposal is below the 9317-slot
  stability P95 estimate and is not silently accepted.
- Made legacy scale trace replay consume the same frozen `replay_wait_ms`
  values without 250/500ms random fallback or 2-second clipping.
- Added per-model Episode counts, token P50/P95, wait P50/P95, pair usage, and
  latency-source usage to replay health and SuiteMetrics reporting.
- Expanded unified SuiteMetrics throughput reporting into submission,
  completion, and successful Episode/s for overall, dataset, mode, and
  dataset-mode views.
- Added completion throughput for every Worker and Worker-dataset record,
  while keeping unavailable Worker-level submission/success counters null
  instead of inferring them from global data.
- Added separate scale, formal-stability, and trace-collection entrypoints.
- Converted the source tree into the `uenv_stress` Python package.
- Split scale, trace-collection, stability, and runtime-host configurations.
- Added a validated, secret-free inventory for the Server and two active Worker
  hosts; the retired Worker is explicitly banned.
- Added stable public facade modules for Episode construction, result handling,
  Worker fleet operations, production guards, and workload builders.
- Moved runtime artifacts out of the source tree by default.
- Removed the plaintext Worker-password CLI default from the SWE trace converter;
  it now requires `UENV_PASS` and uses the fingerprint-checking SSH connector.
- Fixed the refactored scale orchestrator's SWE pressure command to read gateway
  and agent ports from the SWE pressure section instead of an undefined
  `collection` variable.
- Made the dedicated trace-collection scenario report `llm_kind=real` during
  preflight so its real LLM configuration is permission- and hash-checked.
- Preserved the current working tree's multi-Worker implementation as the source
  baseline without editing the legacy directory.
- Made five-dataset coverage mandatory for both scale pressure and formal
  stability: DSCodeBench, SWE-bench Pro, OlymMATH, SciTab, and PubMedQA.
- Added one isolated real Math Worker/plugin scale channel that runs OlymMATH,
  SciTab, and PubMedQA separately across all three parallel modes and at least
  ten capacity waves per dataset/mode.
- Added a fail-closed five-dataset coverage record to the scale-suite summary.
- Reused the rule-task workload adapters between scale and stability while
  keeping their load schedules and acceptance criteria separate.
- Unified scale and stability replay as `round_robin_episode` across all five
  datasets. New Episodes rotate through the collected corpus; multi-turn
  Episodes remain bound to one trajectory and advance within its turns.
- Added replay audit fields for corpus size, Episode assignments, completed
  cycles, next slot, per-trace use, and exhausted-turn reuse.
- Added one strict `EpisodeObservation` schema shared by scale pressure and
  formal stability across all five datasets. Scale clients persist JSONL;
  stability persists the identical columns in SQLite and can export JSONL/CSV.
- Kept exactly one observation per submitted Episode, including missing,
  duplicate-terminal, UEnv-error, late-result, and RPC-error outcomes.
- Reserved Worker/lease attribution fields without inventing values that the
  current Adapter `SampleResult` does not expose.
- Added `SuiteMetrics v1`, shared by scale pressure and formal stability, with
  overall, per-dataset, per-parallel-mode, per-Worker, and
  per-Worker×dataset metrics.
- Added Worker completed-load minimum/mean/P95/maximum/CV, replay hit rate,
  planned-versus-actual submission rate, resource-sampling evidence, cleanup
  evidence, and fail-closed data-quality status.
- Added replay hit/miss counters to the stability replay service and required
  a run-owned isolated Server log for formal per-Worker evidence.

No pressure test, formal stability phase, fault injection, deployment, service
restart, or production mutation was performed.
