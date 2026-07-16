use serde::Serialize;

use crate::config::BenchConfig;

#[derive(Debug, Clone, Serialize)]
pub struct WorkerPlan {
    pub worker_id: String,
    pub endpoint: String,
    pub ordinal: usize,
    pub shard_id: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunPlan {
    pub scenario: String,
    pub run_id: String,
    pub total_workers: usize,
    pub shard_id: usize,
    pub shard_count: usize,
    pub shard_workers: usize,
    pub first_ordinal: usize,
    pub workers: Vec<WorkerPlan>,
}

pub fn build_plan(cfg: &BenchConfig) -> RunPlan {
    let total = cfg.run.workers;
    let shard_count = cfg.loadgen.shard_count;
    let shard_id = cfg.loadgen.shard_id;
    let base = total / shard_count;
    let remainder = total % shard_count;
    let shard_workers = base + usize::from(shard_id < remainder);
    let first_ordinal = shard_id * base + shard_id.min(remainder);
    let workers = (0..shard_workers)
        .map(|offset| {
            let ordinal = first_ordinal + offset;
            let worker_id = format!(
                "{}-{}-s{:02}-{:06}",
                cfg.run.worker_prefix, cfg.run.run_id, shard_id, ordinal
            );
            let endpoint = cfg
                .run
                .endpoint_template
                .replace("{worker_id}", &worker_id)
                .replace("{ordinal}", &ordinal.to_string())
                .replace("{shard_id}", &shard_id.to_string());
            WorkerPlan {
                worker_id,
                endpoint,
                ordinal,
                shard_id,
            }
        })
        .collect();
    RunPlan {
        scenario: cfg.run.scenario.clone(),
        run_id: cfg.run.run_id.clone(),
        total_workers: total,
        shard_id,
        shard_count,
        shard_workers,
        first_ordinal,
        workers,
    }
}

#[cfg(test)]
mod tests {
    use crate::config::BenchConfig;

    use super::*;

    #[test]
    fn distributes_workers_across_shards() {
        let mut cfg = BenchConfig::default();
        cfg.run.workers = 10;
        cfg.run.run_id = "r".into();
        cfg.loadgen.shard_count = 4;
        let counts: Vec<_> = (0..4)
            .map(|shard| {
                cfg.loadgen.shard_id = shard;
                build_plan(&cfg).workers.len()
            })
            .collect();
        assert_eq!(counts, vec![3, 3, 2, 2]);
    }

    #[test]
    fn worker_ids_are_stable() {
        let cfg = BenchConfig::default();
        let plan = build_plan(&cfg);
        assert_eq!(plan.workers[0].worker_id, "bench-local-dry-run-s00-000000");
    }
}
