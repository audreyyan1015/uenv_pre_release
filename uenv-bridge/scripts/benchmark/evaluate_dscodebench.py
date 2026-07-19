#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import sys
import traceback
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

from tqdm import tqdm


DEFAULT_DATA = Path("/data/ronghao/uenv/uenv-bridge/data/benchmarks/dscodebench/DSCodeBench.json")
DEFAULT_OFFICIAL_EVAL_DIR = Path(
    "/data/ronghao/third_party/DSCodeBench/benchmark_construction_evaluation"
)


def load_dataset(
    path: Path,
    *,
    limit: int | None = None,
    library: str | None = None,
    max_per_library: int | None = None,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    per_library: Counter[str] = Counter()
    with path.open("r", encoding="utf-8") as file:
        for line in file:
            if not line.strip():
                continue
            row = json.loads(line)
            if library and row["library"] != library:
                continue
            if max_per_library is not None and per_library[row["library"]] >= max_per_library:
                continue
            rows.append(row)
            per_library[row["library"]] += 1
            if limit is not None and len(rows) >= limit:
                break
    return rows


def build_prompt(problem: str, *, prompt_style: str) -> str:
    if prompt_style == "official":
        return (
            "Please generate Python3 solution for the following code problem description:\n\n"
            "# Code problem description #\n"
            f"{problem}\n\n"
            "# Response #\n"
            "The return should follow the following format (replace {} with the solution). "
            'Do not generate additional code, such as "__main__" block.'
            "Solution:\n{}"
        )
    if prompt_style == "official_fenced":
        return (
            "Please generate Python3 solution for the following code problem description:\n\n"
            "# Code problem description #\n"
            f"{problem}\n\n"
            "# Response #\n"
            'Do not generate additional code, such as "__main__" block. '
            "Return only one Python markdown code block containing the solution code.\n"
            "Solution:\n```python\n"
        )
    raise ValueError(f"unknown prompt_style: {prompt_style}")


def build_messages(problem: str, *, prompt_style: str) -> list[dict[str, str]]:
    return [
        {
            "role": "system",
            "content": "You are a careful Python data science coding assistant.",
        },
        {"role": "user", "content": build_prompt(problem, prompt_style=prompt_style)},
    ]


def generate(args: argparse.Namespace) -> None:
    from transformers import AutoTokenizer
    from vllm import LLM, SamplingParams

    rows = load_dataset(
        args.data,
        limit=args.limit,
        library=args.library,
        max_per_library=args.max_per_library,
    )
    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
    prompts = [
        tokenizer.apply_chat_template(
            build_messages(row["code_problem"], prompt_style=args.prompt_style),
            tokenize=False,
            add_generation_prompt=True,
            **({"enable_thinking": False} if args.disable_thinking else {}),
        )
        for row in rows
    ]

    llm = LLM(
        model=args.model,
        tensor_parallel_size=args.tensor_parallel_size,
        trust_remote_code=True,
        dtype=args.dtype,
        gpu_memory_utilization=args.gpu_memory_utilization,
        max_model_len=args.max_model_len,
        enforce_eager=args.enforce_eager,
    )
    sampling = SamplingParams(
        temperature=args.temperature,
        top_p=args.top_p,
        max_tokens=args.max_tokens,
    )
    outputs = llm.generate(prompts, sampling)

    args.output_dir.mkdir(parents=True, exist_ok=True)
    generations = []
    for idx, (row, output) in enumerate(zip(rows, outputs, strict=True)):
        text = output.outputs[0].text
        generations.append(
            {
                "id": idx,
                "problem_id": row["problem_id"],
                "library": row["library"],
                "response": text,
                "output_tokens": len(output.outputs[0].token_ids),
            }
        )
    (args.output_dir / "generations.json").write_text(
        json.dumps(generations, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps({"generated": len(generations), "output": str(args.output_dir)}, indent=2))


def load_official_eval(eval_dir: Path):
    sys.path.insert(0, str(eval_dir))
    import run_test  # type: ignore

    return run_test


def execute_case_in_child(
    official_eval_dir: str,
    row: dict[str, Any],
    code: str,
    test_case_number: int,
    random_seed: int,
    debug_errors: bool,
    queue: mp.Queue,
) -> None:
    try:
        official = load_official_eval(Path(official_eval_dir))
        is_plot = "output.png" in row["ground_truth_code"]
        inputs, output_gt, output_test = official.get_exec_output(
            row["ground_truth_code"],
            code,
            row["test_script"],
            is_matplotlib_or_seaborn=is_plot,
            test_case_number=test_case_number,
            random_seed=random_seed,
        )
        evaluation_result = official.evaluate_outputs(inputs, output_gt, output_test)
        case_count = len(evaluation_result)
        executed = case_count > 0
        queue.put(
            {
                "executed": executed,
                "passed": executed and all(int(item) == 1 for item in evaluation_result),
                "case_count": case_count,
                "error": "",
            }
        )
    except Exception as exc:  # noqa: BLE001 - benchmark failures are recorded per sample.
        error = f"{type(exc).__name__}: {exc}"
        if debug_errors:
            error += "\n" + traceback.format_exc()
        queue.put({"executed": False, "passed": False, "case_count": 0, "error": error})


def evaluate_case_with_timeout(
    official_eval_dir: Path,
    row: dict[str, Any],
    code: str,
    test_case_number: int,
    random_seed: int,
    timeout_seconds: float,
    debug_errors: bool,
) -> dict[str, Any]:
    if timeout_seconds <= 0:
        queue: mp.Queue = mp.Queue()
        execute_case_in_child(
            str(official_eval_dir),
            row,
            code,
            test_case_number,
            random_seed,
            debug_errors,
            queue,
        )
        return queue.get()

    ctx = mp.get_context("fork")
    queue = ctx.Queue()
    process = ctx.Process(
        target=execute_case_in_child,
        args=(
            str(official_eval_dir),
            row,
            code,
            test_case_number,
            random_seed,
            debug_errors,
            queue,
        ),
    )
    process.start()
    process.join(timeout_seconds)
    if process.is_alive():
        process.terminate()
        process.join(5)
        if process.is_alive():
            process.kill()
            process.join()
        return {
            "executed": False,
            "passed": False,
            "case_count": 0,
            "error": f"TimeoutError: exceeded {timeout_seconds:g}s",
        }
    if not queue.empty():
        return queue.get()
    if process.exitcode == 0:
        return {"executed": False, "passed": False, "case_count": 0, "error": ""}
    return {
        "executed": False,
        "passed": False,
        "case_count": 0,
        "error": f"ProcessError: evaluator exited with code {process.exitcode}",
    }


def safe_div(num: float, den: float) -> float:
    return num / den if den else 0.0


def write_metrics(results: list[dict[str, Any]], output_dir: Path) -> dict[str, Any]:
    total = len(results)
    parsed = sum(1 for row in results if row["parsed"])
    passed = sum(1 for row in results if row["passed"])
    executed = sum(1 for row in results if row["executed"])
    errors = sum(1 for row in results if row["error"])
    by_library: dict[str, dict[str, int]] = defaultdict(
        lambda: {"total": 0, "parsed": 0, "executed": 0, "passed": 0, "errors": 0}
    )
    for row in results:
        stats = by_library[row["library"]]
        stats["total"] += 1
        stats["parsed"] += int(row["parsed"])
        stats["executed"] += int(row["executed"])
        stats["passed"] += int(row["passed"])
        stats["errors"] += int(bool(row["error"]))

    metrics = {
        "problem_count": total,
        "parsed_count": parsed,
        "executed_count": executed,
        "passed_count": passed,
        "error_count": errors,
        "parse_rate": safe_div(parsed, total),
        "execution_rate": safe_div(executed, total),
        "pass_at_1": safe_div(passed, total),
        "library_distribution": dict(Counter(row["library"] for row in results)),
        "by_library": {
            library: {
                "problem_count": values["total"],
                "parse_rate": safe_div(values["parsed"], values["total"]),
                "execution_rate": safe_div(values["executed"], values["total"]),
                "pass_at_1": safe_div(values["passed"], values["total"]),
                "error_count": values["errors"],
            }
            for library, values in sorted(by_library.items())
        },
    }
    (output_dir / "metrics.json").write_text(
        json.dumps(metrics, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return metrics


def evaluate(args: argparse.Namespace) -> None:
    official = load_official_eval(args.official_eval_dir)
    rows = load_dataset(
        args.data,
        limit=args.limit,
        library=args.library,
        max_per_library=args.max_per_library,
    )
    generations = json.loads(args.generations.read_text(encoding="utf-8"))
    by_problem_id = {row["problem_id"]: row for row in generations}

    args.output_dir.mkdir(parents=True, exist_ok=True)
    result_path = args.output_dir / "evaluation_results.jsonl"
    results: list[dict[str, Any]] = []
    with result_path.open("w", encoding="utf-8") as file:
        for idx, row in enumerate(tqdm(rows, desc="evaluating")):
            response = by_problem_id.get(row["problem_id"], {})
            raw_response = str(response.get("response", ""))
            code = official.extract_code(raw_response)
            parsed = bool(code.strip())
            executed = False
            passed = False
            case_count = 0
            error = ""
            if parsed:
                eval_result = evaluate_case_with_timeout(
                    args.official_eval_dir,
                    row,
                    code,
                    args.test_case_number,
                    args.random_seed,
                    args.per_problem_timeout,
                    args.debug_errors,
                )
                executed = bool(eval_result["executed"])
                passed = bool(eval_result["passed"])
                case_count = int(eval_result["case_count"])
                error = str(eval_result["error"])

            result = {
                "id": idx,
                "problem_id": row["problem_id"],
                "library": row["library"],
                "parsed": parsed,
                "executed": executed,
                "passed": passed,
                "case_count": case_count,
                "error": error,
                "output_tokens": response.get("output_tokens"),
            }
            results.append(result)
            file.write(json.dumps(result, ensure_ascii=False) + "\n")
            file.flush()

    metrics = write_metrics(results, args.output_dir)
    print(json.dumps(metrics, ensure_ascii=False, indent=2))


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate and evaluate DSCodeBench solutions.")
    subparsers = parser.add_subparsers(dest="mode", required=True)

    gen = subparsers.add_parser("generate")
    gen.add_argument("--data", type=Path, default=DEFAULT_DATA)
    gen.add_argument("--model", required=True)
    gen.add_argument("--output-dir", type=Path, required=True)
    gen.add_argument("--limit", type=int)
    gen.add_argument("--library")
    gen.add_argument("--max-per-library", type=int)
    gen.add_argument("--tensor-parallel-size", type=int, default=8)
    gen.add_argument("--dtype", default="bfloat16")
    gen.add_argument("--gpu-memory-utilization", type=float, default=0.9)
    gen.add_argument("--max-model-len", type=int, default=8192)
    gen.add_argument("--max-tokens", type=int, default=2048)
    gen.add_argument("--temperature", type=float, default=0.2)
    gen.add_argument("--top-p", type=float, default=1.0)
    gen.add_argument("--prompt-style", choices=("official", "official_fenced"), default="official_fenced")
    gen.add_argument("--disable-thinking", action="store_true")
    gen.add_argument("--enforce-eager", action="store_true")
    gen.set_defaults(func=generate)

    ev = subparsers.add_parser("evaluate")
    ev.add_argument("--data", type=Path, default=DEFAULT_DATA)
    ev.add_argument("--generations", type=Path, required=True)
    ev.add_argument("--output-dir", type=Path, required=True)
    ev.add_argument("--official-eval-dir", type=Path, default=DEFAULT_OFFICIAL_EVAL_DIR)
    ev.add_argument("--limit", type=int)
    ev.add_argument("--library")
    ev.add_argument("--max-per-library", type=int)
    ev.add_argument("--test-case-number", type=int, default=20)
    ev.add_argument("--random-seed", type=int, default=42)
    ev.add_argument(
        "--per-problem-timeout",
        type=float,
        default=120.0,
        help="Wall-clock timeout for one generated solution. Use 0 to disable.",
    )
    ev.add_argument("--debug-errors", action="store_true")
    ev.set_defaults(func=evaluate)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
