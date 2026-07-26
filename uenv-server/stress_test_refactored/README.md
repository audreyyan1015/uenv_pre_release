# UEnv Stress Test Refactored

This directory is an isolated, structured refactor of `uenv-server/stress_test`.
It does not read artifacts from or write artifacts into the legacy source
directory.

## Entrypoints

Run commands from this directory:

```bash
python3 -m uenv_stress.cli.run_scale_suite --help
python3 -m uenv_stress.cli.run_trace_collection --help
python3 -m uenv_stress.cli.run_formal_stability_suite --help
```

Both executable suites require all five datasets: DSCodeBench, SWE-bench Pro,
OlymMATH, SciTab, and PubMedQA. The scale runner gives each dataset independent
1024+ real-Worker, three-parallel-mode, and capacity-wave evidence. The formal
stability runner mixes the same five workloads across
selfcheck/reference/stability/pressure/capacity/burst/fault phases. `pressure`
uses the reference/stability arrival schedule multiplied by exactly 10.

The trace collector prepares frozen real-model inputs. Trace collection is
separate from pressure execution and is never reported as scale evidence.

All trace data lives under `/home/uenv-stress`; datasets, virtual environments,
runtime configuration, and run artifacts remain under `/opt/uenv-stress`.
Compatibility symlinks may exist at the former `/opt/uenv-stress/trace-*`
locations for legacy runners, but the refactored suite never resolves trace
storage through those links.

## Mixed replay semantics

Generate the immutable mixed corpus before formal admission:

```bash
python3 -m uenv_stress.tools.prepare_mixed_stability_corpora
```

DSCodeBench, OlymMATH, SciTab, and PubMedQA contain 100 strict same-question
pairs. Adjacent Episode sequences select Doubao then Qwen from the same
`pair_id`; Qwen surplus rows are ignored. SWE-bench Pro contains exactly 50
Doubao trajectories and no Qwen trajectory.

No model timing calibration is performed. Replay uses an existing recorded
latency when available, otherwise the recorded Episode elapsed time as the
explicitly named `observed_episode_elapsed_proxy`. A multi-turn Episode divides
that proxy in proportion to its actual completion tokens, and its per-turn
waits sum exactly to the Episode proxy. Missing proxies may use the same
dataset/model median only when the missing share is at most 5%; otherwise
formal admission fails closed. The builder freezes this plan into
`replay_wait_ms` so formal stability, formal 10x pressure, and legacy scale
trace replay use the same wait values. Fixed 250/500ms profiles apply only to
non-trace template simulation.

Candidate reference/stability rates are 6.3345, 2.0719, 60.18, 148, and
148 Episode/s respectively; pressure rates are exactly 10 times those values.
The rate basis is the 100xA100 throughput estimate and is deliberately
independent from the latency proxy.

Replay artifacts expose model/family counts, pair selection, latency-source
counts, completion-token P50/P95, and wait P50/P95. The legacy 1024-Worker
drivers remain separate protocol/scheduler/plugin scale evidence; they do not
replace the paced five-task 10x pressure phase.

## Episode observation contract

Scale pressure and formal stability now use one versioned
`EpisodeObservation` row from `uenv_stress.core.result`. Every submitted
Episode produces exactly one row, including missing-result and RPC-error
Episodes, so the observation denominator cannot silently shrink.

The flat schema groups the same fields for all five datasets:

- identity and scenario: suite, run, phase, task/dataset, environment and mode;
- Episode binding: episode/request/batch IDs, sample/sequence and dataset item;
- lifecycle: planned, dispatched, deadline and terminal timestamps plus
  end-to-end and batch RPC latency;
- outcome: status, reward, done, termination, error and failure class;
- workload attribution: Worker/host/lease/attempt and replay trace binding;
- training evidence: steps, response tokens, trace validity, rollout versions
  and result checksum;
- `extensions_json` for scenario-specific values without changing the common
  columns.

Scale clients write `*.episode-observations.jsonl` next to each result and copy
it into the local run directory. Formal stability stores the same columns in
`episode.sqlite`, table `episode`; `--export-episode-csv` additionally emits
both CSV and JSONL.

The current Adapter `SampleResult` does not expose Worker identity or dispatch
lease fields. Those columns therefore remain empty and
`worker_attribution=unavailable_in_adapter_result`; Worker coverage is still
derived separately from isolated Server logs. The schema can accept those
values later without another format migration.

`run_scale_suite` and `run_formal_stability_suite` do not execute a workload
unless their explicit execution flag is supplied. SSH credentials are read
only from `UENV_PASS`.

## Runtime inventory

`uenv_stress/config/runtime_hosts.json` is the authoritative host inventory for
the refactored tree. It lists the two allowed Worker hosts and explicitly bans
the retired host. Secrets and dynamic production PIDs are not stored in it.

## Artifacts

Use an absolute directory under `/opt/uenv-stress/runs/<run-id>`. Do not place
runtime artifacts, bytecode caches, trace corpora, or manual backups in this
source tree.

## Validation

```bash
python3 -m compileall -q uenv_stress tests
python3 -m unittest discover -s tests -v
```

See `REFACTOR_PLAN.md` for boundaries and migration details.
See `EPISODE_OBSERVATION_CN.md` for the complete Chinese description of the
shared Episode observation contract.
See `SUITE_METRICS_CN.md` for the mentor-facing aggregate metrics shared by
scale pressure and formal stability.
