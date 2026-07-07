#!/usr/bin/env python3
"""OpenHands benchmark runner HTTP API (208.77 :8888, health :8777).

两种触发模式并存：
  - HTTP 旁路（原有）：POST /v1/runs 手动/调试触发。
  - Server 编排（新增，OPENHANDS_AGENT_POLL=1）：启动即 RegisterAgent，后台循环
    PollAgentJob 领取 Server 下派的 AgentJob，跑完 CompleteAgentJob 回填 reward。
    此模式下 gateway_url 来自 AgentJob，不再依赖硬编码 UENV_GATEWAY。
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

API_BIND = os.environ.get("OPENHANDS_RUNNER_API_BIND", "0.0.0.0:8888")
HEALTH_BIND = os.environ.get("OPENHANDS_RUNNER_HEALTH_BIND", "0.0.0.0:8777")
RUN_SCRIPT = os.environ.get(
    "OPENHANDS_RUN_SCRIPT", "/root/UEnv/scripts/run-openhands-pro-20877.sh"
)
RUNS_DIR = Path(os.environ.get("OPENHANDS_RUNS_DIR", "/var/log/uenv/openhands-runs"))

# ── Server 编排（Agent 池 poll）模式配置 ────────────────────────────────────
AGENT_POLL_ENABLED = os.environ.get("OPENHANDS_AGENT_POLL", "0") == "1"
SERVER_ENDPOINT = os.environ.get("UENV_SERVER_ENDPOINT", "")
AGENT_POOL_ID = os.environ.get("OPENHANDS_AGENT_POOL_ID", "openhands-default")
AGENT_ID = os.environ.get("OPENHANDS_AGENT_ID", "")  # 空则注册时由 Server 生成
AGENT_BRIDGE_ID = os.environ.get("OPENHANDS_AGENT_BRIDGE_ID", "uenv-agent-openhands")
AGENT_BRIDGE_VERSION = os.environ.get("OPENHANDS_AGENT_BRIDGE_VERSION", "1.0.0")
AGENT_MAX_CONCURRENT = int(os.environ.get("OPENHANDS_AGENT_MAX_CONCURRENT", "1"))
POLL_INTERVAL_SEC = float(os.environ.get("OPENHANDS_POLL_INTERVAL_SEC", "3"))
HEARTBEAT_INTERVAL_SEC = float(os.environ.get("OPENHANDS_HEARTBEAT_INTERVAL_SEC", "10"))
# uenv_runtime 包所在目录（agent_client / agent_job），默认 monorepo 路径。
BRIDGE_DIR = os.environ.get("UENV_AGENT_BRIDGE_DIR", "/root/UEnv/integrations/openhands")
# 路由标签，格式 "k1=v1,k2=v2"（如 "region=bj,gpu=a100"），供 Server 多池标签亲和用。
AGENT_LABELS = os.environ.get("OPENHANDS_AGENT_LABELS", "")


def _parse_labels(raw: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for pair in raw.split(","):
        pair = pair.strip()
        if "=" in pair:
            k, v = pair.split("=", 1)
            k = k.strip()
            if k:
                out[k] = v.strip()
    return out

_lock = threading.Lock()
_jobs: dict[str, dict[str, Any]] = {}
_stop = threading.Event()
_active_jobs = 0  # 当前在跑的 AgentJob 数（心跳上报用）
_active_lock = threading.Lock()
_registration_lock = threading.Lock()



def _parse_bind(bind: str) -> tuple[str, int]:
    host, _, port = bind.rpartition(":")
    return host or "0.0.0.0", int(port or "8080")


def _run_job(job_id: str, mode: str, max_iterations: int, instance: str | None) -> None:
    env = os.environ.copy()
    if instance:
        env["UENV_PRO_INSTANCE"] = instance
    env["MAX_ITERATIONS"] = str(max_iterations)
    cmd = ["bash", RUN_SCRIPT, mode]
    with _lock:
        _jobs[job_id]["status"] = "running"
        _jobs[job_id]["started_at"] = time.time()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=int(os.environ.get("OPENHANDS_RUN_TIMEOUT_SEC", "7200")),
            check=False,
        )
        with _lock:
            _jobs[job_id]["status"] = "succeeded" if proc.returncode == 0 else "failed"
            _jobs[job_id]["exit_code"] = proc.returncode
            _jobs[job_id]["stdout"] = proc.stdout[-8000:]
            _jobs[job_id]["stderr"] = proc.stderr[-8000:]
            _jobs[job_id]["finished_at"] = time.time()
    except subprocess.TimeoutExpired as exc:
        with _lock:
            _jobs[job_id]["status"] = "timeout"
            _jobs[job_id]["stderr"] = str(exc)[-8000:]
            _jobs[job_id]["finished_at"] = time.time()


class ApiHandler(BaseHTTPRequestHandler):
    server_version = "openhands-runner/1.0"

    def log_message(self, fmt: str, *args: Any) -> None:
        print(f"[runner-api] {self.address_string()} {fmt % args}", flush=True)

    def _json(self, code: int, body: dict[str, Any]) -> None:
        raw = json.dumps(body, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/health":
            self._json(200, {"status": "ok", "service": "openhands-runner"})
            return
        if path.startswith("/v1/runs/"):
            job_id = path.rsplit("/", 1)[-1]
            with _lock:
                job = _jobs.get(job_id)
            if not job:
                self._json(404, {"error": "run not found", "id": job_id})
                return
            self._json(200, job)
            return
        self._json(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path != "/v1/runs":
            self._json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw.decode("utf-8") or "{}")
        except json.JSONDecodeError:
            self._json(400, {"error": "invalid json"})
            return
        mode = str(payload.get("mode", "gold")).lower()
        if mode not in {"gold", "llm"}:
            self._json(400, {"error": "mode must be gold or llm"})
            return
        max_iterations = int(payload.get("max_iterations", 30))
        instance = payload.get("instance")
        job_id = str(uuid.uuid4())
        job = {
            "id": job_id,
            "mode": mode,
            "max_iterations": max_iterations,
            "instance": instance,
            "status": "queued",
            "created_at": time.time(),
        }
        with _lock:
            _jobs[job_id] = job
        threading.Thread(
            target=_run_job,
            args=(job_id, mode, max_iterations, instance),
            daemon=True,
        ).start()
        self._json(202, {"id": job_id, "status": "queued"})


class HealthHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args: Any) -> None:
        print(f"[runner-health] {self.address_string()} {fmt % args}", flush=True)

    def do_GET(self) -> None:  # noqa: N802
        body = b'{"status":"ok","service":"openhands-runner"}\n'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def _serve(name: str, bind: str, handler: type[BaseHTTPRequestHandler]) -> None:
    host, port = _parse_bind(bind)
    httpd = HTTPServer((host, port), handler)
    print(f"[{name}] listening on {host}:{port}", flush=True)
    httpd.serve_forever()


# ═══════════════════════════════════════════════════════════════════════════
# Server 编排（Agent 池 poll）模式
# ═══════════════════════════════════════════════════════════════════════════

def _import_agent_client():
    """把 BRIDGE_DIR 加入 sys.path 后导入 uenv_runtime.agent_client。"""
    if BRIDGE_DIR and BRIDGE_DIR not in sys.path:
        sys.path.insert(0, BRIDGE_DIR)
    # 生成的 stub 目录也需可导入（其内部扁平 import agent_pb2）。
    gen_dir = os.path.join(BRIDGE_DIR, "uenv_runtime", "gen")
    if os.path.isdir(gen_dir) and gen_dir not in sys.path:
        sys.path.insert(0, gen_dir)
    from uenv_runtime.agent_client import AgentControlClient  # noqa: PLC0415

    return AgentControlClient


def _read_reward(out_dir: Path) -> tuple[str, float, str]:
    """从 driver 输出的 submit_result.json 读 (status, reward, trajectory_id)。

    submit_result.json 结构见 run_swebenchpro_official.py：含 reward / resolved /
    trajectory_ref.trajectory_id。文件缺失（driver 崩溃）视为 failed。
    """
    f = out_dir / "submit_result.json"
    if not f.is_file():
        return "failed", 0.0, ""
    try:
        doc = json.loads(f.read_text(encoding="utf-8"))
    except Exception:  # noqa: BLE001
        return "failed", 0.0, ""
    reward = float(doc.get("reward", 0.0) or 0.0)
    ref = doc.get("trajectory_ref") or {}
    tid = ref.get("trajectory_id") if isinstance(ref, dict) else None
    trajectory_id = str(tid) if tid else ""  # null/缺失 → 空串，不要变成 "None"
    # driver 正常退出即视为 completed（reward 承载评分；resolved 与否由 reward 反映）。
    status = "completed"
    return status, reward, trajectory_id


def _run_agent_job(client: Any, job: Any) -> None:
    """跑一个 AgentJob：写 job 文件 → 调 run 脚本 → 读结果 → CompleteAgentJob。

    全程包在 try/finally 内：无论 setup（mkdir/write）还是执行阶段抛错，都保证
    _active_jobs 被回收（否则 max_concurrent=1 时 poller 会永久卡死），并尽力把
    结果回填 Server。
    """
    global _active_jobs
    status, reward, trajectory_id, err = "failed", 0.0, "", ""
    try:
        stamp = time.strftime("%Y%m%d-%H%M%S")
        out_dir = RUNS_DIR / f"agent-{job.job_id}-{stamp}"
        out_dir.mkdir(parents=True, exist_ok=True)
        job_file = out_dir / "agent_job.json"
        # AgentJob dataclass → JSON（driver 通过 UENV_AGENT_JOB_FILE 读取并覆盖 gateway 等）。
        job_dict = dict(job.__dict__)
        # 208.77 经 SSH 隧道访问 7143 Gateway 时，将 Server 下发的公网 URL 换成本机隧道口。
        local_gw = os.environ.get("UENV_GATEWAY_LOCAL", "").strip()
        if local_gw and job_dict.get("gateway_url"):
            print(
                f"[agent-poll] gateway rewrite {job_dict['gateway_url']} -> {local_gw}",
                flush=True,
            )
            job_dict["gateway_url"] = local_gw
        if not job_dict.get("gateway_api_key"):
            job_dict["gateway_api_key"] = os.environ.get("UENV_GATEWAY_API_KEY", "swe-pro-secret")
        job_file.write_text(json.dumps(job_dict, indent=2) + "\n", encoding="utf-8")

        env = os.environ.copy()
        env["UENV_AGENT_JOB_FILE"] = str(job_file)
        env["MAX_ITERATIONS"] = str(job.max_iterations or 30)
        env["OPENHANDS_OUT_DIR"] = str(out_dir)  # 让脚本把输出写到可预测目录
        if job.run_id:
            env["UENV_RUN_ID"] = job.run_id
        # gateway 由 AgentJob 注入，显式清掉环境里可能残留的硬编码值。
        env.pop("UENV_GATEWAY", None)

        mode = job.mode if job.mode in ("gold", "llm") else "llm"
        try:
            proc = subprocess.run(
                ["bash", RUN_SCRIPT, mode],
                capture_output=True,
                text=True,
                env=env,
                timeout=int(os.environ.get("OPENHANDS_RUN_TIMEOUT_SEC", "7200")),
                check=False,
            )
            (out_dir / "runner_stdout.log").write_text(proc.stdout[-16000:], encoding="utf-8")
            (out_dir / "runner_stderr.log").write_text(proc.stderr[-16000:], encoding="utf-8")
            if proc.returncode == 0:
                status, reward, trajectory_id = _read_reward(out_dir)
            else:
                status = "failed"
                err = f"run script exit {proc.returncode}: {proc.stderr[-2000:]}"
        except subprocess.TimeoutExpired as exc:
            status, err = "timeout", str(exc)[-2000:]
    except Exception as exc:  # noqa: BLE001
        # setup（mkdir/write）或其他意外失败也回填 failed，不吞掉。
        status, err = "failed", f"{type(exc).__name__}: {exc}"[-2000:]
    finally:
        try:
            acked = client.complete_agent_job(
                job_id=job.job_id,
                run_id=job.run_id,
                status=status,
                reward=reward,
                trajectory_id=trajectory_id,
                error_message=err,
            )
            print(
                f"[agent-poll] completed job={job.job_id} status={status} "
                f"reward={reward} trajectory_id={trajectory_id} acked={acked}",
                flush=True,
            )
        except Exception as exc:  # noqa: BLE001
            print(f"[agent-poll] CompleteAgentJob failed job={job.job_id}: {exc}", flush=True)
        with _active_lock:
            _active_jobs -= 1



def _mark_agent_unregistered(agent_state: dict[str, Any], reason: str) -> None:
    with _registration_lock:
        was_registered = bool(agent_state.get("registered"))
        agent_state["registered"] = False
    if was_registered:
        print(f"[agent-poll] registration invalidated: {reason}", flush=True)


def _register_agent_once(
    client: Any,
    agent_state: dict[str, Any],
    bridges: list[dict[str, str]],
    labels: dict[str, str],
) -> bool:
    with _registration_lock:
        requested_agent_id = str(agent_state.get("agent_id") or AGENT_ID)
    try:
        agent_id = client.register_agent(
            agent_id=requested_agent_id,
            agent_pool_id=AGENT_POOL_ID,
            synced_bridges=bridges,
            max_concurrent=AGENT_MAX_CONCURRENT,
            labels=labels,
        )
    except Exception as exc:  # noqa: BLE001
        print(f"[agent-poll] RegisterAgent failed, retrying: {exc}", flush=True)
        return False
    with _registration_lock:
        agent_state["agent_id"] = agent_id
        agent_state["registered"] = True
    print(
        f"[agent-poll] registered agent_id={agent_id} pool={AGENT_POOL_ID} "
        f"max_concurrent={AGENT_MAX_CONCURRENT}",
        flush=True,
    )
    return True


def _heartbeat_loop(client: Any, agent_state: dict[str, Any]) -> None:
    while not _stop.is_set():
        with _registration_lock:
            agent_id = str(agent_state.get("agent_id") or "")
            registered = bool(agent_state.get("registered"))
        if not registered or not agent_id:
            _stop.wait(HEARTBEAT_INTERVAL_SEC)
            continue
        try:
            with _active_lock:
                active = _active_jobs
            client.agent_heartbeat(agent_id, active, int(time.time() * 1000))
        except Exception as exc:  # noqa: BLE001
            print(f"[agent-poll] heartbeat failed: {exc}", flush=True)
            _mark_agent_unregistered(agent_state, "heartbeat_failed")
        _stop.wait(HEARTBEAT_INTERVAL_SEC)


def _poll_loop() -> None:
    global _active_jobs
    if not SERVER_ENDPOINT:
        print("[agent-poll] UENV_SERVER_ENDPOINT unset; poll mode disabled", flush=True)
        return
    try:
        AgentControlClient = _import_agent_client()
        # __init__ ???? grpc + stub??? grpcio / ??? stub ??????
        # ?? catch????? poll ?????? HTTP ???
        client = AgentControlClient(SERVER_ENDPOINT)
    except Exception as exc:  # noqa: BLE001
        print(f"[agent-poll] cannot init agent client (poll mode off): {exc}", flush=True)
        return

    bridges = [{"package_id": AGENT_BRIDGE_ID, "version": AGENT_BRIDGE_VERSION, "bundle_digest": ""}]
    labels = _parse_labels(AGENT_LABELS)
    agent_state: dict[str, Any] = {"agent_id": AGENT_ID, "registered": False}

    # ?????????? Server ?????? heartbeat/poll ???? registered
    # ????????? RegisterAgent??? Server ?????????????
    while not _stop.is_set():
        if _register_agent_once(client, agent_state, bridges, labels):
            break
        _stop.wait(POLL_INTERVAL_SEC)
    if _stop.is_set():
        return

    threading.Thread(target=_heartbeat_loop, args=(client, agent_state), daemon=True).start()

    while not _stop.is_set():
        with _registration_lock:
            registered = bool(agent_state.get("registered"))
            agent_id = str(agent_state.get("agent_id") or "")
        if not registered:
            if not _register_agent_once(client, agent_state, bridges, labels):
                _stop.wait(POLL_INTERVAL_SEC)
            continue

        # ?????????? poll?? Server try_reserve ????????
        with _active_lock:
            busy = _active_jobs >= AGENT_MAX_CONCURRENT
        if busy:
            _stop.wait(POLL_INTERVAL_SEC)
            continue
        try:
            job = client.poll_agent_job(AGENT_POOL_ID, agent_id)
        except Exception as exc:  # noqa: BLE001
            print(f"[agent-poll] PollAgentJob failed: {exc}", flush=True)
            _mark_agent_unregistered(agent_state, "poll_failed")
            _stop.wait(POLL_INTERVAL_SEC)
            continue
        if job is None:
            _stop.wait(POLL_INTERVAL_SEC)
            continue
        print(
            f"[agent-poll] got job={job.job_id} instance={job.instance_id} "
            f"mode={job.mode} gateway={job.gateway_url}",
            flush=True,
        )
        with _active_lock:
            _active_jobs += 1
        threading.Thread(target=_run_agent_job, args=(client, job), daemon=True).start()


def main() -> None:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    threading.Thread(
        target=_serve, args=("health", HEALTH_BIND, HealthHandler), daemon=True
    ).start()
    if AGENT_POLL_ENABLED:
        print("[agent-poll] Server orchestration mode enabled", flush=True)
        threading.Thread(target=_poll_loop, daemon=True).start()
    _serve("api", API_BIND, ApiHandler)


if __name__ == "__main__":
    main()

