#!/usr/bin/env python3
"""DSCodeBench official-style single-problem harness (adapted from run_test.py).

Upstream batch evaluator: ShuyinOuyang/DSCodeBench
`benchmark_construction_evaluation/run_test.py`.

This module exposes a *single-problem* API suitable for UEnv CodeEnv episodes:
generate tests via `test_case_input_generator`, execute ground-truth then
candidate, compare outputs.
"""

from __future__ import annotations

import ast
import math
import os
import re
import tempfile
from typing import Any


def extract_code(text: str) -> str:
    blocks = re.findall(r"```(?:[\w]+)?\n(.*?)```", text, re.DOTALL)
    return blocks[0] if blocks else text


def classify_code_ast(code: str) -> dict[str, Any]:
    classified: dict[str, Any] = {
        "imports": [],
        "functions": {},
        "classes": {},
        "class_methods": {},
        "others": "",
    }
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return classified
    for node in tree.body:
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            classified["imports"].append(ast.unparse(node))
        elif isinstance(node, ast.ClassDef):
            classified["classes"][node.name] = ast.unparse(node)
            for inside in node.body:
                if isinstance(inside, ast.FunctionDef):
                    classified["class_methods"][f"{node.name}.{inside.name}"] = ast.unparse(inside)
                elif isinstance(inside, ast.AsyncFunctionDef):
                    classified["class_methods"][f"{node.name}.async_{inside.name}"] = ast.unparse(
                        inside
                    )
        elif isinstance(node, ast.FunctionDef):
            classified["functions"][node.name] = ast.unparse(node)
        elif isinstance(node, ast.AsyncFunctionDef):
            classified["functions"][node.name] = ast.unparse(node)
        else:
            classified["others"] += ast.unparse(node) + "\n"
    return classified


def extract_imports(code: str) -> list[tuple[str, str]]:
    imports: list[tuple[str, str]] = []
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return imports
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append((alias.name, alias.asname or alias.name))
        elif isinstance(node, ast.ImportFrom):
            module = node.module or ""
            for alias in node.names:
                imports.append((f"{module}.{alias.name}", alias.asname or alias.name))
    return imports


def get_library_from_code(code: str) -> list[tuple[str, str]]:
    classified = classify_code_ast(code)
    libs: list[tuple[str, str]] = []
    for import_line in classified["imports"]:
        libs += extract_imports(import_line.strip())
    return libs


def get_main_function_name_and_parameter_count_brief(code: str) -> tuple[str | None, int | None]:
    try:
        tree = ast.parse(code)
        functions = [
            node
            for node in tree.body
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        ]
        if not functions:
            return None, None
        last = functions[-1]
        return last.name, len(last.args.args)
    except SyntaxError:
        return None, None


def add_random_seed_code(import_libs: list[tuple[str, str]], random_seed: int) -> str:
    random_seed_code = ""
    for import_lib in import_libs:
        name = import_lib[0]
        if "random" in name:
            random_seed_code += f"import random\nrandom.seed({random_seed})\n"
        elif name in [
            "numpy",
            "pandas",
            "scipy",
            "matplotlib",
            "matplotlib.pyplot",
            "seaborn",
        ]:
            random_seed_code += f"import numpy as np\nnp.random.seed({random_seed})\n"
        elif "torch" in name:
            random_seed_code += (
                "import random\nimport numpy as np\nimport torch\n"
                f"torch.manual_seed({random_seed})\n"
                f"random.seed({random_seed})\n"
                f"np.random.seed({random_seed})\n"
            )
        elif name in ["tensorflow", "keras"]:
            random_seed_code += (
                "import tensorflow as tf\nimport random\nimport numpy as np\n"
                f"tf.random.set_seed({random_seed})\n"
                f"random.seed({random_seed})\n"
                f"np.random.seed({random_seed})\n"
            )
    return random_seed_code


def add_random_seed_into_functions(code: str, random_seed: int) -> str:
    class AddRandomStateTransformer(ast.NodeTransformer):
        def __init__(self) -> None:
            self.api_list = [
                "RatioUniforms",
                "RandomForestClassifier",
                "RandomForestRegressor",
                "KFold",
                "StratifiedKFold",
                "LinearSVC",
                "MLPRegressor",
                "train_test_split",
                "make_regression",
                "make_classification",
            ]

        def visit_Call(self, node: ast.Call) -> ast.AST:
            self.generic_visit(node)
            for randomness_api in self.api_list:
                match = (
                    isinstance(node.func, ast.Name) and node.func.id == randomness_api
                ) or (
                    isinstance(node.func, ast.Attribute) and node.func.attr == randomness_api
                )
                if not match:
                    continue
                if randomness_api in ("KFold", "StratifiedKFold"):
                    shuffle_exist = False
                    for keyword in node.keywords:
                        if keyword.arg == "shuffle":
                            keyword.value = ast.Constant(value=True)
                            shuffle_exist = True
                        if keyword.arg == "random_state":
                            keyword.value = ast.Constant(value=random_seed)
                            break
                    else:
                        if not shuffle_exist:
                            node.keywords.append(
                                ast.keyword(arg="shuffle", value=ast.Constant(value=True))
                            )
                        node.keywords.append(
                            ast.keyword(arg="random_state", value=ast.Constant(value=random_seed))
                        )
                else:
                    for keyword in node.keywords:
                        if keyword.arg == "random_state":
                            keyword.value = ast.Constant(value=random_seed)
                            break
                    else:
                        node.keywords.append(
                            ast.keyword(arg="random_state", value=ast.Constant(value=random_seed))
                        )
            return node

    try:
        tree = ast.parse(code)
        tree = AddRandomStateTransformer().visit(tree)
        ast.fix_missing_locations(tree)
        return ast.unparse(tree)
    except Exception:
        return code


def add_matplotlib_agg(code: str) -> str:
    lines = code.splitlines()
    result: list[str] = []
    added_agg = False
    for line in lines:
        result.append(line)
        if "matplotlib" in line and "import" in line and not added_agg:
            result.append("import matplotlib\nmatplotlib.use('Agg')")
            added_agg = True
    return "\n".join(result)


def get_additional_code(
    code: str, is_matplotlib_or_seaborn: bool, test_case_number: int = 0
) -> str:
    n = "" if test_case_number == 0 else str(test_case_number)
    main_function_name, function_parameter_count = get_main_function_name_and_parameter_count_brief(
        code
    )
    if main_function_name is None:
        raise ValueError("no top-level function found in candidate/ground-truth code")

    if is_matplotlib_or_seaborn:
        call = (
            f"{main_function_name}(test_cases[i])"
            if function_parameter_count is None or function_parameter_count <= 2
            else f"{main_function_name}(*test_cases[i])"
        )
        return f"""
from PIL import Image
import numpy as np

test_cases = test_case_input_generator({n})
output_list = []
for i in range(len(test_cases)):
    {call}
    img = np.array(Image.open("output.png").convert("RGB"))
    output_list.append(img)
"""

    if function_parameter_count == 1:
        return f"""
test_cases = test_case_input_generator({n})
output_list = []
for i in range(len(test_cases)):
    output_list.append({main_function_name}(test_cases[i]))
"""
    return f"""
test_cases = test_case_input_generator({n})
output_list = []
for i in range(len(test_cases)):
    output_list.append({main_function_name}(*test_cases[i]))
"""


def prepare_exec_code(
    code: str,
    test_case_script: str,
    *,
    is_ground_truth_code: bool = True,
    random_seed: int = 42,
    is_matplotlib_or_seaborn: bool = False,
    test_case_number: int = 0,
) -> str:
    import_libs = get_library_from_code(code)
    try:
        code = add_random_seed_into_functions(code, random_seed)
    except Exception:
        pass
    code = add_matplotlib_agg(code)
    test_case_script = add_random_seed_into_functions(test_case_script, random_seed)
    if is_ground_truth_code:
        import_libs += get_library_from_code(test_case_script)
    import_libs = list(set(import_libs))
    random_seed_code = add_random_seed_code(import_libs, random_seed)
    additional_code = get_additional_code(code, is_matplotlib_or_seaborn, test_case_number)
    return code + "\n\n" + test_case_script + "\n\n" + random_seed_code + "\n\n" + additional_code


def get_code_output_list(
    exec_code: str, test_case_input_list: list[Any] | None = None
) -> tuple[list[Any], list[Any]]:
    local_namespace: dict[str, Any] = {}
    if test_case_input_list is not None:
        local_namespace["test_cases"] = test_case_input_list
    with tempfile.TemporaryDirectory(prefix="uenv-dscode-") as tmp_dir:
        old_dir = os.getcwd()
        try:
            os.chdir(tmp_dir)
            exec(exec_code, local_namespace)  # noqa: S102 — intentional harness exec
            outputs = local_namespace.get("output_list", [])
            inputs = local_namespace.get("test_cases", [])
            return inputs, outputs
        except Exception:
            return [], []
        finally:
            os.chdir(old_dir)


def get_exec_output(
    ground_truth_code: str,
    test_solution_code: str,
    test_case_script: str,
    *,
    is_matplotlib_or_seaborn: bool = False,
    test_case_number: int = 200,
    random_seed: int = 42,
) -> tuple[list[Any], list[Any], list[Any]]:
    exec_gt = prepare_exec_code(
        ground_truth_code,
        test_case_script,
        is_ground_truth_code=True,
        random_seed=random_seed,
        is_matplotlib_or_seaborn=is_matplotlib_or_seaborn,
        test_case_number=test_case_number,
    )
    inputs, gt_outputs = get_code_output_list(exec_gt)

    exec_sol = prepare_exec_code(
        test_solution_code,
        test_case_script,
        is_ground_truth_code=True,
        random_seed=random_seed,
        is_matplotlib_or_seaborn=is_matplotlib_or_seaborn,
        test_case_number=test_case_number,
    )
    _, sol_outputs = get_code_output_list(exec_sol, test_case_input_list=inputs)
    return inputs, gt_outputs, sol_outputs


def _allclose(a: Any, b: Any) -> bool:
    try:
        import numpy as np

        return bool(np.allclose(a, b, equal_nan=True))
    except Exception:
        return False


def values_equal(result: Any, ans: Any) -> bool:
    """Compare harness outputs; heavy libs are optional."""
    try:
        if isinstance(result, tuple):
            if not isinstance(ans, tuple) or len(result) != len(ans):
                return False
            return all(values_equal(result[i], ans[i]) for i in range(len(result)))
        if isinstance(result, list):
            if not isinstance(ans, list) or len(result) != len(ans):
                return False
            return all(values_equal(result[i], ans[i]) for i in range(len(result)))
        if isinstance(result, dict):
            if not isinstance(ans, dict) or set(result) != set(ans):
                return False
            return all(values_equal(result[k], ans[k]) for k in result)

        try:
            import numpy as np

            if isinstance(result, np.ndarray) or isinstance(ans, np.ndarray):
                return _allclose(result, ans)
            if isinstance(result, np.ma.MaskedArray):
                return bool(np.ma.allclose(result, ans))
        except ImportError:
            pass

        try:
            import pandas as pd

            if isinstance(result, pd.DataFrame):
                pd.testing.assert_frame_equal(result, ans)
                return True
        except Exception:
            if "pandas" in type(result).__module__:
                return False

        try:
            import torch

            if isinstance(result, torch.Tensor):
                return bool(torch.allclose(result, ans, equal_nan=True))
        except Exception:
            if "torch" in type(result).__module__:
                return False

        if isinstance(result, float) or isinstance(ans, float):
            if result is None or ans is None:
                return result is ans
            if math.isnan(result) and math.isnan(ans):
                return True
            return math.isclose(float(result), float(ans), rel_tol=1e-6, abs_tol=1e-8)

        return result == ans
    except Exception:
        return False


def evaluate_outputs(
    ground_truth_outputs: list[Any], solution_outputs: list[Any]
) -> list[int]:
    if not ground_truth_outputs or not solution_outputs:
        return []
    n = min(len(ground_truth_outputs), len(solution_outputs))
    results: list[int] = []
    for i in range(n):
        results.append(1 if values_equal(solution_outputs[i], ground_truth_outputs[i]) else 0)
    return results


def evaluate_problem(
    *,
    ground_truth_code: str,
    candidate_code: str,
    test_script: str,
    num_tests: int = 200,
    random_seed: int = 42,
) -> dict[str, Any]:
    """Run official-style evaluation for one DSCodeBench problem."""
    is_plot = "output.png" in ground_truth_code
    candidate_code = extract_code(candidate_code).strip() or candidate_code
    if not candidate_code.strip():
        return {
            "passed": False,
            "tests_run": 0,
            "tests_passed": 0,
            "error": "empty candidate code",
        }
    if not ground_truth_code.strip():
        return {
            "passed": False,
            "tests_run": 0,
            "tests_passed": 0,
            "error": "empty ground_truth_code",
        }
    if "test_case_input_generator" not in test_script:
        return {
            "passed": False,
            "tests_run": 0,
            "tests_passed": 0,
            "error": "test_script missing test_case_input_generator",
        }

    try:
        _inputs, gt_out, sol_out = get_exec_output(
            ground_truth_code,
            candidate_code,
            test_script,
            is_matplotlib_or_seaborn=is_plot,
            test_case_number=num_tests,
            random_seed=random_seed,
        )
    except Exception as exc:  # noqa: BLE001
        return {
            "passed": False,
            "tests_run": 0,
            "tests_passed": 0,
            "error": f"harness exec failed: {exc}",
        }

    if not gt_out:
        return {
            "passed": False,
            "tests_run": 0,
            "tests_passed": 0,
            "error": "ground-truth produced no outputs (syntax/runtime error?)",
        }
    if not sol_out:
        return {
            "passed": False,
            "tests_run": len(gt_out),
            "tests_passed": 0,
            "error": "candidate produced no outputs (syntax/runtime error?)",
        }

    flags = evaluate_outputs(gt_out, sol_out)
    tests_run = len(flags)
    tests_passed = sum(flags)
    return {
        "passed": tests_run > 0 and tests_passed == tests_run,
        "tests_run": tests_run,
        "tests_passed": tests_passed,
        "error": None if tests_run > 0 and tests_passed == tests_run else "some tests failed",
    }
