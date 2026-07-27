#!/usr/bin/env python3
"""DSCodeBench ToolEnv Agent（Verifiers 风格 run_python + submit_code）。

与单轮基线 (evaluate_dscodebench_uenv.py, pass@1) 的区别：
  * 基线：Adapter→Core→Worker code env→Model Gateway 单轮生成 → 官方 harness 判分。
  * ToolEnv：Agent 端多轮 (run_python 本地沙箱迭代/自测 → submit_code 定稿)，
    定稿代码经**同一** code env inline_harness 判分（agentic_pass@1）。

无控制面改动的接法：Worker 会向 `model_endpoint.url` 拉取候选代码再判分，
因此本脚本在 submit_code 后启动一个 OpenAI 兼容 shim，返回 Agent 定稿代码，
把 EpisodeRequest.model_endpoint 指向该 shim → Worker 用官方 harness 判 Agent 的代码。

部署位置：可运行在 208.77（Agent 机）。shim 需 Worker(7143) 可达：
  * 本机联调：--shim-host 127.0.0.1（Worker 同机）。
  * 208.77：--shim-host 0.0.0.0 --shim-public-url http://8.130.208.77:8888/v1（Worker 出站可达公网口）。

用法示例（7143 本机 smoke, mock=ground_truth 确定性打通）：
  cd /root/UEnv && PYTHONPATH=uenv-bridge/src \
    python3 uenv-bridge/scripts/benchmark/dscode_toolenv_agent.py \
      --endpoint 8.130.75.157:8088 --limit 2 --policy mock \
      --shim-host 127.0.0.1 --shim-port 18899
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))
SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient
from uenv.bridge.protocol import EpisodeResult

# evaluate_dscodebench 顶部硬 import tqdm；缺失时注入 shim（本脚本仅用其 build_prompt/load_dataset）。
try:  # pragma: no cover
    import tqdm  # noqa: F401
except ModuleNotFoundError:  # pragma: no cover
    import types as _types

    _tqdm_mod = _types.ModuleType("tqdm")
    _tqdm_mod.tqdm = lambda iterable=None, *a, **k: (iterable if iterable is not None else [])
    sys.modules.setdefault("tqdm", _tqdm_mod)

from evaluate_dscodebench import build_prompt, load_dataset
from evaluate_dscodebench_uenv import bool_from_info, build_request


# --------------------------------------------------------------------------
# run_python 工具：Agent 本地受限沙箱（在 Agent 机执行，用于自测/迭代）。
# --------------------------------------------------------------------------
def run_python(
    code: str,
    *,
    stdin: str = "",
    timeout_secs: int = 30,
    python_bin: str | None = None,
) -> dict[str, Any]:
    py = python_bin or sys.executable
    with tempfile.TemporaryDirectory(prefix="toolenv-") as tmp:
        src = Path(tmp) / "snippet.py"
        src.write_text(code, encoding="utf-8")
        try:
            proc = subprocess.run(
                [py, str(src)],
                input=stdin,
                capture_output=True,
                text=True,
                timeout=timeout_secs,
                cwd=tmp,
            )
            return {
                "stdout": proc.stdout[-8000:],
                "stderr": proc.stderr[-8000:],
                "exit_code": proc.returncode,
                "timed_out": False,
                "python": py,
            }
        except subprocess.TimeoutExpired as exc:
            return {
                "stdout": (exc.stdout or "")[-8000:] if isinstance(exc.stdout, str) else "",
                "stderr": f"TIMEOUT after {timeout_secs}s",
                "exit_code": -1,
                "timed_out": True,
                "python": py,
            }


# --------------------------------------------------------------------------
# 策略（Policy）：决定每一轮调用 run_python 还是 submit_code。
# --------------------------------------------------------------------------
class MockGroundTruthPolicy:
    """确定性打通用：第 1 轮 run_python 自测 ground_truth，第 2 轮 submit_code 定稿。

    用于免-LLM 验证 ToolEnv 全链路（run_python→submit_code→官方 harness→reward），
    与 qa 的 rule_reward 免-LLM smoke 同性质，不代表真实智能。
    """

    def act(self, prompt: str, row: dict[str, Any], history: list[dict[str, Any]]) -> dict[str, Any]:
        code = str(row.get("ground_truth_code", "")).strip() or "def solve():\n    return None\n"
        if not history:
            probe = code + "\nprint('toolenv_probe_ok')\n"
            return {"tool": "run_python", "code": probe}
        return {"tool": "submit_code", "code": code}


class OpenAICompatiblePolicy:
    """真实 LLM 策略：调用 OpenAI 兼容 /chat/completions。

    简化协议：首轮直接生成代码并（可选）run_python 自测一次，随后 submit_code。
    需要 --llm-endpoint / --llm-model（HTTPS 端点需 --llm-api-key）。
    """

    def __init__(self, endpoint: str, model: str, api_key: str = "", max_tokens: int = 1024, temperature: float = 0.2):
        import urllib.request  # noqa: F401 (used in _chat)

        self.endpoint = endpoint.rstrip("/")
        self.model = model
        self.api_key = api_key
        self.max_tokens = max_tokens
        self.temperature = temperature

    def _chat(self, messages: list[dict[str, str]]) -> str:
        import urllib.request

        body = json.dumps(
            {
                "model": self.model,
                "messages": messages,
                "max_tokens": self.max_tokens,
                "temperature": self.temperature,
                # Qwen3：关闭 thinking，避免吃掉小样本预算
                "chat_template_kwargs": {"enable_thinking": False},
            }
        ).encode()
        req = urllib.request.Request(f"{self.endpoint}/chat/completions", data=body, method="POST")
        req.add_header("Content-Type", "application/json")
        if self.api_key:
            req.add_header("Authorization", f"Bearer {self.api_key}")
        with urllib.request.urlopen(req, timeout=180) as resp:
            data = json.loads(resp.read().decode())
        msg = data["choices"][0]["message"]
        content = msg.get("content") or ""
        # 部分后端把 thinking 放在单独字段；content 为空时回退
        if not content and msg.get("reasoning_content"):
            content = str(msg["reasoning_content"])
        return content

    @staticmethod
    def _strip_thinking(text: str) -> str:
        # 去掉 <think>...</think> / <thinking>...</thinking>
        import re

        return re.sub(r"<think(?:ing)?>[\s\S]*?</think(?:ing)?>", "", text, flags=re.I).strip()

    @classmethod
    def _extract_code(cls, text: str) -> str:
        text = cls._strip_thinking(text)
        if "```" in text:
            parts = text.split("```")
            for i in range(1, len(parts), 2):
                block = parts[i]
                if block.lstrip().startswith("python"):
                    block = block.lstrip()[len("python") :]
                elif block.lstrip().startswith("py"):
                    block = block.lstrip()[len("py") :]
                return block.strip()
        return text.strip()

    def act(self, prompt: str, row: dict[str, Any], history: list[dict[str, Any]]) -> dict[str, Any]:
        if not history:
            content = self._chat(
                [
                    {
                        "role": "system",
                        "content": (
                            "You are a Python coding agent for data-science problems. "
                            "Return a complete, runnable module in a single ```python code block. "
                            "Put ALL helper functions at module top-level (never nest them). "
                            "Match the exact function names and signatures required by the problem. "
                            "Do not explain outside the code block."
                        ),
                    },
                    {"role": "user", "content": prompt},
                ]
            )
            code = self._extract_code(content)
            # 语法 + 顶层 def 自检：嵌套 helper 会导致官方 harness 找不到符号
            probe = (
                "import ast\n"
                f"src={code!r}\n"
                "tree=ast.parse(src)\n"
                "tops=[n.name for n in tree.body if isinstance(n, ast.FunctionDef)]\n"
                "print('TOPLEVEL_DEFS', tops)\n"
                "print('toolenv_probe_ok')\n"
            )
            return {"tool": "run_python", "code": probe, "solution_code": code}

        last = history[-1]
        last_code = last.get("solution_code") or last.get("code", "")
        last_code = last_code.replace("\nprint('toolenv_probe_ok')\n", "\n")
        obs = last.get("observation") or {}
        # 若 run_python 失败且还有轮次预算，让 LLM 根据 stderr 修一次再定稿
        failed = bool(obs.get("timed_out")) or int(obs.get("exit_code", 0) or 0) != 0
        repair_done = any(h.get("tool") == "run_python" and h.get("repaired") for h in history)
        if failed and not repair_done:
            content = self._chat(
                [
                    {
                        "role": "system",
                        "content": (
                            "Fix the Python module based on the error. "
                            "Keep ALL helpers at module top-level. Return only a ```python code block."
                        ),
                    },
                    {
                        "role": "user",
                        "content": (
                            f"Original problem:\n{prompt}\n\nCurrent code:\n```python\n{last_code}\n```\n\n"
                            f"stdout:\n{obs.get('stdout','')}\nstderr:\n{obs.get('stderr','')}\n"
                            f"exit_code={obs.get('exit_code')}"
                        ),
                    },
                ]
            )
            fixed = self._extract_code(content)
            probe = (
                "import ast\n"
                f"src={fixed!r}\n"
                "tree=ast.parse(src)\n"
                "tops=[n.name for n in tree.body if isinstance(n, ast.FunctionDef)]\n"
                "print('TOPLEVEL_DEFS', tops)\n"
                "print('toolenv_probe_ok')\n"
            )
            return {"tool": "run_python", "code": probe, "solution_code": fixed, "repaired": True}

        return {"tool": "submit_code", "code": last_code}


# --------------------------------------------------------------------------
# submit_code 判分 shim：OpenAI 兼容，返回 Agent 定稿代码作为 completion。
# --------------------------------------------------------------------------
class _CodeState:
    def __init__(self) -> None:
        self.code = ""
        self.lock = threading.Lock()

    def set(self, code: str) -> None:
        with self.lock:
            self.code = code

    def get(self) -> str:
        with self.lock:
            return self.code


def _make_handler(state: _CodeState):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):  # 静默
            return

        def _send(self, obj: dict[str, Any], status: int = 200) -> None:
            body = json.dumps(obj).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # /v1/models 探活兼容
            self._send({"data": [{"id": "toolenv-shim"}]})

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0") or "0")
            _ = self.rfile.read(length)
            code = state.get()
            self._send(
                {
                    "id": f"chatcmpl-{uuid.uuid4().hex[:12]}",
                    "object": "chat.completion",
                    "model": "toolenv-shim",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": code},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
                }
            )

    return Handler


def start_shim(host: str, port: int, state: _CodeState) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer((host, port), _make_handler(state))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


# --------------------------------------------------------------------------
# ToolEnv 主循环
# --------------------------------------------------------------------------
def run_toolenv_episode(
    policy,
    prompt: str,
    row: dict[str, Any],
    *,
    max_turns: int,
    run_python_timeout: int,
    python_bin: str | None = None,
) -> tuple[str, list[dict[str, Any]]]:
    history: list[dict[str, Any]] = []
    final_code = ""
    for _turn in range(max_turns):
        action = policy.act(prompt, row, history)
        tool = action.get("tool")
        code = action.get("code", "")
        if tool == "submit_code":
            final_code = code
            history.append({"tool": "submit_code", "code": code})
            break
        if tool == "run_python":
            obs = run_python(code, timeout_secs=run_python_timeout, python_bin=python_bin)
            entry = {"tool": "run_python", "code": code, "observation": obs}
            if action.get("solution_code"):
                entry["solution_code"] = action["solution_code"]
            if action.get("repaired"):
                entry["repaired"] = True
            history.append(entry)
            continue
        # 未知工具：直接定稿当前代码，避免死循环。
        final_code = code
        history.append({"tool": "submit_code", "code": code})
        break
    if not final_code and history:
        last = history[-1]
        final_code = last.get("solution_code") or last.get("code", "")
    return final_code, history


def build_policy(args) -> Any:
    if args.policy == "mock":
        return MockGroundTruthPolicy()
    if args.policy == "llm":
        if not args.llm_endpoint or not args.llm_model:
            raise SystemExit("policy=llm 需要 --llm-endpoint 与 --llm-model")
        return OpenAICompatiblePolicy(
            endpoint=args.llm_endpoint,
            model=args.llm_model,
            api_key=args.llm_api_key,
            max_tokens=args.max_tokens,
        )
    raise SystemExit(f"unknown policy: {args.policy}")


TRACK = "agentic"
METRIC_NAME = "agentic_pass@1"


def _resolve_output_dir(args) -> Path | None:
    """固定输出目录布局；`--output` 保留兼容（取其父目录）。"""
    if args.output_dir:
        return Path(args.output_dir)
    if args.output:
        return Path(args.output).parent
    return None


def _load_done(results_jsonl: Path) -> dict[str, dict[str, Any]]:
    """读取已完成的 problem_id → row，用于 --resume 续跑。"""
    done: dict[str, dict[str, Any]] = {}
    if not results_jsonl.exists():
        return done
    for line in results_jsonl.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        pid = str(row.get("problem_id", ""))
        if pid:
            done[pid] = row
    return done


def _aggregate(rows: list[dict[str, Any]]) -> dict[str, Any]:
    by_library: dict[str, dict[str, Any]] = {}
    for row in rows:
        lib = str(row.get("library") or "unknown")
        stats = by_library.setdefault(lib, {"total": 0, "passed": 0})
        stats["total"] += 1
        stats["passed"] += 1 if row.get("passed") else 0
    for stats in by_library.values():
        stats["agentic_pass_at_1"] = stats["passed"] / stats["total"] if stats["total"] else 0.0

    turns = [int(row.get("toolenv_turns") or 0) for row in rows]
    calls = [int(row.get("run_python_calls") or 0) for row in rows]
    n = len(rows)
    passed = sum(1 for row in rows if row.get("passed"))
    return {
        "problem_count": n,
        "passed_count": passed,
        "agentic_pass_at_1": (passed / n) if n else 0.0,
        "completed_count": sum(1 for row in rows if row.get("status") == "completed"),
        "avg_toolenv_turns": (sum(turns) / n) if n else 0.0,
        "avg_run_python_calls": (sum(calls) / n) if n else 0.0,
        "by_library": {lib: by_library[lib] for lib in sorted(by_library)},
    }


def main() -> int:
    p = argparse.ArgumentParser(description="DSCodeBench ToolEnv Agent (run_python + submit_code)")
    p.add_argument("--endpoint", default="8.130.75.157:8088", help="Adapter Core gRPC endpoint")
    p.add_argument("--data", default=None, help="DSCodeBench 数据集 json（默认仓库内置）")
    p.add_argument("--limit", type=int, default=2)
    p.add_argument("--library", default=None, help="仅评测该库的题目（numpy/pandas/...）")
    p.add_argument("--max-per-library", type=int, default=None)
    p.add_argument("--prompt-style", default="official")
    p.add_argument("--policy", choices=["mock", "llm"], default="mock")
    p.add_argument("--max-turns", type=int, default=4)
    p.add_argument("--run-python-timeout", type=int, default=30)
    p.add_argument(
        "--python-bin",
        default=None,
        help="run_python 使用的解释器；默认 sys.executable。7143 建议指向 DSCodeBench venv",
    )
    # shim（判分回传通道）
    p.add_argument("--shim-host", default="127.0.0.1")
    p.add_argument("--shim-port", type=int, default=18899)
    p.add_argument("--shim-public-url", default=None, help="Worker 可达的 shim URL；默认 http://<shim-host>:<port>/v1")
    # code env 判分参数（与基线对齐）
    p.add_argument("--evaluation-mode", default="inline_harness", choices=["inline_harness", "path_harness"])
    p.add_argument("--num-tests", type=int, default=200)
    p.add_argument("--seed", type=int, default=1234)
    p.add_argument("--code-timeout-secs", type=int, default=300)
    p.add_argument("--timeout-seconds", type=int, default=600)
    p.add_argument("--client-timeout-seconds", type=int, default=900)
    # llm policy
    p.add_argument("--llm-endpoint", default=None)
    p.add_argument("--llm-model", default=None)
    p.add_argument("--llm-api-key", default="")
    p.add_argument("--max-tokens", type=int, default=1024)
    # 报告产出
    p.add_argument(
        "--output-dir",
        default=None,
        help="固定输出目录：metrics.json / results.jsonl / codes/ / traces/ / run_config.json",
    )
    p.add_argument("--output", default=None, help="兼容旧用法：metrics.json 全路径（其父目录作为 output-dir）")
    p.add_argument("--run-name", default=None, help="报告里的 run 标识；默认由 policy+model+时间戳生成")
    p.add_argument("--resume", action="store_true", help="跳过 results.jsonl 中已完成的题目")
    p.add_argument(
        "--require-all-pass",
        action="store_true",
        help="smoke 用：仅当全部题目通过才返回 0；正式评测不要开（评测低分不是失败）",
    )
    args = p.parse_args()

    data_path = Path(args.data) if args.data else (ROOT / "data/benchmarks/dscodebench/DSCodeBench.json")
    examples = load_dataset(
        data_path,
        limit=args.limit,
        library=args.library,
        max_per_library=args.max_per_library,
    )
    policy = build_policy(args)

    started_at = time.time()
    run_name = args.run_name or "-".join(
        filter(None, ["toolenv", args.policy, (args.llm_model or "").replace("/", "_") or None, time.strftime("%Y%m%d_%H%M%S")])
    )
    out_dir = _resolve_output_dir(args)
    results_jsonl: Path | None = None
    done: dict[str, dict[str, Any]] = {}
    if out_dir:
        (out_dir / "codes").mkdir(parents=True, exist_ok=True)
        (out_dir / "traces").mkdir(parents=True, exist_ok=True)
        results_jsonl = out_dir / "results.jsonl"
        if args.resume:
            done = _load_done(results_jsonl)
        elif results_jsonl.exists():
            results_jsonl.unlink()
        (out_dir / "run_config.json").write_text(
            json.dumps(
                {
                    "track": TRACK,
                    "metric": METRIC_NAME,
                    "run_name": run_name,
                    "args": {k: v for k, v in vars(args).items() if k != "llm_api_key"},
                    "data": str(data_path),
                    "started_at": started_at,
                },
                ensure_ascii=False,
                indent=2,
            ),
            encoding="utf-8",
        )

    state = _CodeState()
    shim = start_shim(args.shim_host, args.shim_port, state)
    shim_host_for_url = "127.0.0.1" if args.shim_host in ("0.0.0.0", "") else args.shim_host
    shim_url = args.shim_public_url or f"http://{shim_host_for_url}:{args.shim_port}/v1"

    client = RustCoreEpisodeClient(
        RustCoreClientConfig(endpoint=args.endpoint, timeout_seconds=args.client_timeout_seconds, auto_start=False)
    )
    batch_id = f"toolenv-{uuid.uuid4().hex[:8]}"
    rows_out: list[dict[str, Any]] = []
    try:
        for idx, row in enumerate(examples):
            pid = str(row["problem_id"])
            if pid in done:
                rows_out.append(done[pid])
                print(json.dumps({**done[pid], "resumed": True}, ensure_ascii=False), flush=True)
                continue
            episode_started = time.time()
            prompt = build_prompt(row["code_problem"], prompt_style=args.prompt_style)
            final_code, history = run_toolenv_episode(
                policy,
                prompt,
                row,
                max_turns=args.max_turns,
                run_python_timeout=args.run_python_timeout,
                python_bin=args.python_bin,
            )
            state.set(final_code)
            turns = len(history)
            ran = sum(1 for h in history if h["tool"] == "run_python")
            # 落盘定稿代码与轨迹，便于评测复盘
            if out_dir:
                (out_dir / "codes" / f"{pid}.py").write_text(final_code, encoding="utf-8")
                (out_dir / "traces" / f"{pid}.history.json").write_text(
                    json.dumps(history, ensure_ascii=False, indent=2)[:200000], encoding="utf-8"
                )

            request = build_request(
                row=row,
                prompt=prompt,
                sample_index=idx,
                batch_id=batch_id,
                model_endpoint=shim_url,
                model_name="toolenv-shim",
                max_tokens=args.max_tokens,
                temperature=0.0,
                top_p=1.0,
                enable_thinking=False,
                preserve_thinking=False,
                thinking_token_budget=None,
                num_tests=args.num_tests,
                timeout_seconds=args.timeout_seconds,
                code_timeout_secs=args.code_timeout_secs,
                seed=args.seed + idx,
                evaluation_mode=args.evaluation_mode,
            )
            results = list(client.submit_episode_stream([request]))
            result: EpisodeResult = results[0]
            step = result.trajectory.steps[-1] if result.trajectory.steps else None
            info = step.info if step else {}
            reward = float(result.summary.total_reward or 0.0)
            passed_flag = bool_from_info(info.get("passed"))
            passed = passed_flag if passed_flag is not None else reward > 0.0
            row_out = {
                "problem_id": pid,
                "library": row.get("library", ""),
                "toolenv_turns": turns,
                "run_python_calls": ran,
                "status": result.status,
                "reward": reward,
                "passed": passed,
                "tests_run": info.get("tests_run", ""),
                "tests_passed": info.get("tests_passed", ""),
                "worker_error": info.get("error", ""),
                "worker_detail": (info.get("detail", "") or "")[:300],
                "final_code_chars": len(final_code),
                "shim_url": shim_url,
                "elapsed_seconds": round(time.time() - episode_started, 3),
            }
            rows_out.append(row_out)
            if results_jsonl is not None:
                with results_jsonl.open("a", encoding="utf-8") as fh:
                    fh.write(json.dumps(row_out, ensure_ascii=False) + "\n")
            print(json.dumps(row_out, ensure_ascii=False), flush=True)
    finally:
        client.close()
        shim.shutdown()

    metrics = {
        # 分轨标识：本报告是 Agent 轨道，禁止与官方单轮 pass@1 直接比较。
        "track": TRACK,
        "metric": METRIC_NAME,
        "comparable_with_official_pass_at_1": False,
        "run_name": run_name,
        "endpoint": args.endpoint,
        "policy": args.policy,
        "llm_model": args.llm_model or "",
        "llm_endpoint": args.llm_endpoint or "",
        "max_turns": args.max_turns,
        "evaluation_mode": args.evaluation_mode,
        "num_tests": args.num_tests,
        "dataset": str(data_path),
        "library_filter": args.library or "",
        "started_at": started_at,
        "elapsed_seconds": round(time.time() - started_at, 3),
        **_aggregate(rows_out),
        "results": rows_out,
    }
    print(json.dumps({k: v for k, v in metrics.items() if k != "results"}, ensure_ascii=False, indent=2))
    if out_dir:
        (out_dir / "metrics.json").write_text(json.dumps(metrics, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"report: {out_dir / 'metrics.json'}")
    n = metrics["problem_count"]
    if args.require_all_pass:
        ok = n > 0 and metrics["passed_count"] == n
        print("OK: dscode toolenv e2e passed" if ok else "PARTIAL/FAIL: see metrics")
        return 0 if ok else 1
    print(f"DONE: {METRIC_NAME}={metrics['agentic_pass_at_1']:.4f} ({metrics['passed_count']}/{n})")
    return 0 if n > 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
