#!/usr/bin/env python3
"""
轨迹存储压力测试：1 个 adapter-core(:8077 轨迹服务) + N 个并发上传者
模拟多 worker 同时上传轨迹，验证 SQLite WAL 单写连接池在并发下的吞吐/延迟/一致性。

重点验证（冻结方案 v2.2 §9 / §11）：
  - 设计目标 ≤32 并发 upload（脚本默认压到 64，超目标压测）
  - 零数据丢失：acked 数 == 期望数 == 磁盘 body 文件数 == DB 行数
  - 无"有行无文件 / 有文件无行"
  - 幂等：并发重复 POST 同一条 → 一条新增其余 duplicate，无 409
  - gzip 上传往返一致

零外部依赖（urllib + sqlite3 + gzip + concurrent.futures 均为标准库）。
用法：python3 /home/uenv/uenv-server/stress_test/trajectory_stress_test.py
"""

import os
import sys
import time
import json
import gzip
import uuid
import signal
import sqlite3
import subprocess
import urllib.request
import urllib.error
from concurrent.futures import ThreadPoolExecutor, as_completed

# ── 配置 ─────────────────────────────────────────────────────────────────────
TRAJ_PORT       = 18078
LISTEN          = f"127.0.0.1:{TRAJ_PORT}"
BASE_URL        = f"http://127.0.0.1:{TRAJ_PORT}/control/v1/trajectories"
TOKEN           = "stress-token"
DATA_DIR        = "/tmp/trj-stress"

N_CONCURRENT    = int(os.environ.get("TRJ_CONC", "64"))    # 并发上传线程数（>32 设计目标）
TOTAL           = int(os.environ.get("TRJ_TOTAL", "3000")) # 总上传轨迹数
STEPS_PER_TRAJ  = int(os.environ.get("TRJ_STEPS", "40"))   # 每条轨迹步数（决定 body 大小）
GZIP_RATIO      = 0.5    # 一半用 gzip 上传
RUN_ID          = f"run-stress-{uuid.uuid4().hex[:8]}"
DUP_FANOUT      = 8      # 幂等测试：同一条并发重发次数

# ── 路径 ─────────────────────────────────────────────────────────────────────
ADAPTER_BIN = "/home/uenv/target/debug/uenv-adapter-core"   # release(6/21)无轨迹功能，用 debug
DB_PATH     = f"{DATA_DIR}/trajectory.db"
BODIES_DIR  = f"{DATA_DIR}/bodies"
LOG_PATH    = "/home/uenv/uenv-server/stress_test/logs/trajectory_stress.log"

os.makedirs(os.path.dirname(LOG_PATH), exist_ok=True)


# ── 构造一条轨迹 bundle ───────────────────────────────────────────────────────
def make_bundle(tid: str, steps: int) -> bytes:
    bundle = {
        "trajectory_id": tid,
        "run_id": RUN_ID,
        "session_id": f"sess-{tid}",
        "instance_id": f"inst-{int(tid.split('-')[-1]) % 50}",  # 50 个 instance 轮转
        "benchmark_variant": "pro",
        "worker_id": f"w{int(tid.split('-')[-1]) % N_CONCURRENT}",
        "gateway_base_url": "http://127.0.0.1:28999",
        "steps": [
            {
                "step_index": i,
                "action": {"kind": "exec", "command": f"pytest test_{i}.py -x"},
                "observation": {"stdout": "x" * 80, "exit_code": 0},
                "timestamp_ms": 1000 + i,
                "duration_ms": 12,
            }
            for i in range(steps)
        ],
        "artifact": {"episode_id": f"sess-{tid}", "instance_id": "inst", "reward": 1.0},
        "reward": 1.0 if int(tid.split("-")[-1]) % 3 == 0 else 0.0,
        "resolved": int(tid.split("-")[-1]) % 3 == 0,
        "sealed_at_ms": 1700000000000 + int(tid.split("-")[-1]),
    }
    return json.dumps(bundle).encode()


def post(body: bytes, use_gzip: bool, timeout=30):
    """单次 POST，返回 (http_code, duplicate_flag, latency_ms, error_str)。"""
    headers = {"Content-Type": "application/json", "X-Trajectory-Token": TOKEN}
    data = body
    if use_gzip:
        data = gzip.compress(body)
        headers["Content-Encoding"] = "gzip"
    req = urllib.request.Request(BASE_URL, data=data, headers=headers, method="POST")
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            lat = (time.time() - t0) * 1000
            payload = json.loads(resp.read().decode())
            return resp.status, payload.get("duplicate", False), lat, None
    except urllib.error.HTTPError as e:
        lat = (time.time() - t0) * 1000
        return e.code, False, lat, e.read().decode()[:80]
    except Exception as e:
        lat = (time.time() - t0) * 1000
        return 0, False, lat, str(e)[:80]


def get(tid: str, timeout=30):
    req = urllib.request.Request(f"{BASE_URL}/{tid}", headers={"X-Trajectory-Token": TOKEN})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, b""


def list_by_run(run_id: str, limit: int, timeout=60):
    url = f"{BASE_URL}?run_id={run_id}&limit={limit}"
    req = urllib.request.Request(url, headers={"X-Trajectory-Token": TOKEN})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode()).get("trajectories", [])


def percentile(sorted_lat, p):
    if not sorted_lat:
        return 0.0
    return sorted_lat[min(len(sorted_lat) - 1, int(len(sorted_lat) * p))]


def wait_health(timeout_s=30):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"{BASE_URL}/health", timeout=2) as r:
                if r.status == 200:
                    return True
        except Exception:
            time.sleep(0.5)
    return False


def main():
    # 清理旧数据
    subprocess.run(["rm", "-rf", DATA_DIR], check=False)

    print(f"启动 adapter-core（轨迹服务 {LISTEN}）")
    env = os.environ.copy()
    env.update({
        "UENV_ADDR": "127.0.0.1:50552",         # 避开默认 50051
        "UENV_TRAJECTORY_ENABLED": "1",
        "UENV_TRAJECTORY_HTTP_LISTEN": LISTEN,
        "UENV_TRAJECTORY_DATA_DIR": DATA_DIR,
        "UENV_TRAJECTORY_TOKEN": TOKEN,
        "RUST_LOG": "warn",
    })
    log = open(LOG_PATH, "w")
    proc = subprocess.Popen([ADAPTER_BIN], env=env, stdout=log, stderr=log)
    print(f"adapter-core PID={proc.pid}, 等待 :{TRAJ_PORT} 就绪...")
    if not wait_health():
        print("FATAL: 轨迹服务未就绪，见日志", LOG_PATH)
        proc.terminate()
        sys.exit(1)

    print(f"开始压测：TOTAL={TOTAL} 条, 并发={N_CONCURRENT}, 每条 {STEPS_PER_TRAJ} 步, "
          f"gzip 比例={GZIP_RATIO:.0%}")

    # ── 阶段 1：并发上传 ──────────────────────────────────────────────────────
    bodies = {f"trj-stress-{i:06d}": make_bundle(f"trj-stress-{i:06d}", STEPS_PER_TRAJ)
              for i in range(TOTAL)}
    sample_body_kb = len(next(iter(bodies.values()))) / 1024

    latencies, acked, dup, errors = [], 0, 0, 0
    err_samples = []
    t_start = time.time()
    with ThreadPoolExecutor(max_workers=N_CONCURRENT) as ex:
        futs = {
            ex.submit(post, body, (idx % 2 == 0) and (GZIP_RATIO > 0)): tid
            for idx, (tid, body) in enumerate(bodies.items())
        }
        for fut in as_completed(futs):
            code, is_dup, lat, err = fut.result()
            latencies.append(lat)
            if code == 200:
                acked += 1
                if is_dup:
                    dup += 1
            else:
                errors += 1
                if len(err_samples) < 5:
                    err_samples.append(f"{code}:{err}")
    upload_elapsed = time.time() - t_start

    # ── 阶段 2：幂等并发（同一条重发 DUP_FANOUT 次）──────────────────────────
    dup_tid = "trj-stress-000000"
    dup_body = bodies[dup_tid]
    with ThreadPoolExecutor(max_workers=DUP_FANOUT) as ex:
        dup_results = list(ex.map(lambda _: post(dup_body, False), range(DUP_FANOUT)))
    dup_codes = [r[0] for r in dup_results]
    dup_dups = sum(1 for r in dup_results if r[1])
    idem_ok = all(c == 200 for c in dup_codes)  # 全 200，无 409

    # ── 阶段 3：GET 抽样 + LIST 聚合 ─────────────────────────────────────────
    sample_ids = list(bodies.keys())[:: max(1, TOTAL // 20)]  # 抽 ~20 条
    get_ok = 0
    get_mismatch = 0
    for tid in sample_ids:
        code, got = get(tid)
        if code == 200:
            get_ok += 1
            if json.loads(got)["trajectory_id"] != tid:
                get_mismatch += 1
    LIST_CAP = 1000  # server 端 limit clamp(1,1000)；超量需用 since_ms 游标翻页
    listed = list_by_run(RUN_ID, limit=TOTAL + 10)
    list_count = len(listed)
    list_expected = min(TOTAL, LIST_CAP)

    # ── 停服后做磁盘/DB 一致性校验 ──────────────────────────────────────────
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
    log.close()

    body_files = len([f for f in os.listdir(BODIES_DIR) if f.endswith(".json")]) if os.path.isdir(BODIES_DIR) else 0
    con = sqlite3.connect(f"file:{DB_PATH}?mode=ro", uri=True)
    db_rows = con.execute("SELECT COUNT(*) FROM trajectories WHERE upload_status='acked'").fetchone()[0]
    db_present = con.execute("SELECT COUNT(*) FROM trajectories WHERE body_present=1").fetchone()[0]
    # 交叉核对：每个 DB 行的 body 文件是否真存在（无"有行无文件"）
    rows = con.execute("SELECT trajectory_id, body_path FROM trajectories").fetchall()
    missing_files = sum(1 for _, bp in rows if not os.path.exists(os.path.join(DATA_DIR, bp)))
    con.close()

    expected_unique = TOTAL
    lat_sorted = sorted(latencies)
    tput = TOTAL / upload_elapsed if upload_elapsed > 0 else 0

    def mark(ok):
        return "✓ PASS" if ok else "✗ FAIL"

    no_loss = (acked == TOTAL and db_rows == expected_unique and body_files == expected_unique)
    consistent = (missing_files == 0 and db_present == db_rows)

    print(f"""
{'='*64}
  轨迹存储压力测试报告
{'='*64}
  [配置]
    总轨迹数:        {TOTAL}
    并发上传:        {N_CONCURRENT}   (设计目标 ≤32, 本次超目标压测)
    单条 body:       {sample_body_kb:.1f} KB ({STEPS_PER_TRAJ} 步)

  [吞吐 / 延迟]   (debug 构建，release 会更快)
    上传耗时:        {upload_elapsed:.2f}s
    吞吐量:          {tput:.0f} 条/s
    延迟 p50:        {percentile(lat_sorted,0.50):.1f}ms
    延迟 p95:        {percentile(lat_sorted,0.95):.1f}ms
    延迟 p99:        {percentile(lat_sorted,0.99):.1f}ms
    延迟 max:        {lat_sorted[-1] if lat_sorted else 0:.1f}ms

  [正确性]
    HTTP 200(acked): {acked}/{TOTAL}        {mark(acked==TOTAL)}
    HTTP 错误:       {errors}              {mark(errors==0)}
    {('错误样本: ' + '; '.join(err_samples)) if err_samples else ''}

  [零数据丢失]   {mark(no_loss)}
    acked 数:        {acked}
    DB acked 行:     {db_rows}
    磁盘 body 文件:  {body_files}
    (三者应都 = {expected_unique})

  [存储一致性]   {mark(consistent)}
    有行无文件:      {missing_files}        {mark(missing_files==0)}
    body_present=1:  {db_present}/{db_rows}

  [幂等(并发重发 {DUP_FANOUT}x 已存在轨迹)]   {mark(idem_ok and dup_dups==DUP_FANOUT)}
    返回码:          {dup_codes}
    duplicate=true:  {dup_dups}/{DUP_FANOUT}   (期望全部 {DUP_FANOUT}, 该轨迹阶段1已入库)
    无 409 冲突:     {mark(idem_ok)}

  [读取]
    GET 抽样:        {get_ok}/{len(sample_ids)} 成功, {get_mismatch} 内容不符  {mark(get_mismatch==0)}
    LIST by run_id:  {list_count}  (期望 {list_expected}=min(总数,1000上限))   {mark(list_count==list_expected)}
{'='*64}
  日志: {LOG_PATH}
{'='*64}""")

    overall = (no_loss and consistent and idem_ok and dup_dups == DUP_FANOUT
               and errors == 0 and list_count == list_expected and get_mismatch == 0)
    print("  总判定:", "✓✓✓ ALL PASS" if overall else "✗ 有失败项，见上")
    sys.exit(0 if overall else 1)


if __name__ == "__main__":
    main()
