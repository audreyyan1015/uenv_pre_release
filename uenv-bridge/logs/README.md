# UEnv Bridge Logs

`logs/` is only for curated evidence that should be reviewed or shared. Normal script output must stay under `temp/logs/`, which is ignored by git.

Recommended layout:

```text
logs/
  README.md
  curated/
    <run_id>/
      summary.md
      key.log
      metrics.csv
      plots/
```

Default runtime layout:

```text
temp/logs/
  layer4_distributed/<run_id>/
    agent-loop-requests.jsonl
    agent-loop-results.jsonl
  verl_layer4_agent_loop/
    <run_id>.log
    hydra_<run_id>/
```

When a run is worth keeping, copy only the minimum evidence into `logs/curated/<run_id>/`: a short summary, key log excerpts, metrics CSV, and a few plots if needed. Do not commit raw Hydra directories or full routine run logs.
