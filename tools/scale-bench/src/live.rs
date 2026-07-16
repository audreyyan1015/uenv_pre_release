use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use uenv_server::proto::scheduler::v1::control_plane_service_client::ControlPlaneServiceClient;
use uenv_server::proto::scheduler::v1::{
    HeartbeatRequest, RegisterWorkerRequest, SyncedEnvPackage,
};
use uenv_server::proto::v1::ResourceSpec;

use crate::config::BenchConfig;
use crate::plan::{build_plan, WorkerPlan};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub async fn run_live(cfg: BenchConfig) -> Result<()> {
    let plan = build_plan(&cfg);
    let mut client = ControlPlaneServiceClient::connect(cfg.server.grpc_addr.clone())
        .await
        .with_context(|| format!("failed to connect {}", cfg.server.grpc_addr))?;

    let mut accepted = 0usize;
    let register_delay = Duration::from_micros(1_000_000 / cfg.run.register_rps.max(1) as u64);
    for worker in &plan.workers {
        let response = client
            .register_worker(Request::new(register_request(&cfg, worker)))
            .await
            .with_context(|| format!("register_worker failed for {}", worker.worker_id))?
            .into_inner();
        if response.accepted {
            accepted += 1;
        }
        tokio::time::sleep(register_delay).await;
    }

    println!(
        "{}",
        serde_json::json!({
            "event": "register_complete",
            "scenario": cfg.run.scenario,
            "run_id": cfg.run.run_id,
            "shard_id": cfg.loadgen.shard_id,
            "accepted": accepted,
            "attempted": plan.workers.len(),
        })
    );

    if cfg.run.duration_secs == 0 {
        return Ok(());
    }

    let mut tasks = Vec::with_capacity(plan.workers.len());
    for worker in plan.workers {
        let worker_cfg = cfg.clone();
        let mut hb_client = client.clone();
        tasks.push(tokio::spawn(async move {
            heartbeat_loop(&mut hb_client, &worker_cfg, worker).await
        }));
    }

    tokio::time::sleep(Duration::from_secs(cfg.run.duration_secs)).await;
    for task in tasks {
        task.abort();
    }
    Ok(())
}

fn register_request(cfg: &BenchConfig, worker: &WorkerPlan) -> RegisterWorkerRequest {
    RegisterWorkerRequest {
        worker_id: worker.worker_id.clone(),
        supported_env_types: cfg.run.supported_env_types.clone(),
        resource: Some(ResourceSpec {
            cpu_cores: 1,
            memory_mb: 512,
            gpu_count: 0,
            gpu_type: String::new(),
        }),
        endpoint: worker.endpoint.clone(),
        max_concurrent: cfg.run.max_load.max(1) as u32,
        gateway_public_url: String::new(),
        synced_env_packages: Vec::<SyncedEnvPackage>::new(),
        load: 0,
        max_load: cfg.run.max_load,
    }
}

async fn heartbeat_loop(
    client: &mut ControlPlaneServiceClient<tonic::transport::Channel>,
    cfg: &BenchConfig,
    worker: WorkerPlan,
) -> Result<()> {
    let (tx, rx) = mpsc::channel(4);
    let response = client
        .worker_heartbeat(Request::new(ReceiverStream::new(rx)))
        .await
        .with_context(|| format!("heartbeat stream failed for {}", worker.worker_id))?;
    let mut inbound = response.into_inner();
    let interval = Duration::from_millis(cfg.run.heartbeat_interval_ms.max(100));
    let max_load = cfg.run.max_load;
    let worker_id = worker.worker_id.clone();

    let send_task = tokio::spawn(async move {
        loop {
            let req = HeartbeatRequest {
                worker_id: worker_id.clone(),
                load: 0,
                max_load,
                timestamp_ms: now_ms(),
                server_epoch: 0,
            };
            if tx.send(req).await.is_err() {
                break;
            }
            tokio::time::sleep(interval).await;
        }
    });

    while let Some(next) = inbound.message().await? {
        if !next.ok {
            break;
        }
    }
    send_task.abort();
    Ok(())
}
