#!/usr/bin/env python3
"""qa 任务环境的金标判分规则包（verifiers 风格）。

这份文件就是「可证明的金标」本体：它不是报告、不是指标，而是**规则**。
`verify_qa_rubric_alignment.py` 用它来给语料打参照分，Hub 把它作为
`rubric_scorer` 制品托管（`qa-rubric-scorer@<version>`），
`RubricSpec.reference_scorer` 用 sha256 把它钉住。

之所以要单独成文件、并且经 Hub 分发，理由是具体的：
`backend = "verifiers+math_verify"` 只说明用了哪个库，**没有**说明抽取规则。
而 qa 的判分结果几乎完全由抽取规则决定——GSM8K 用官方 `####` 标记还是用
`MathRubric` 默认的 boxed-only parser，会让同一份语料的分数从「基本全对」
变成「全判 0」。两台机器可以都声称「跑的是 verifiers rubric」而实际给出
不同的 reward，且这种分歧在 reward 对不上之前是不可见的。
把规则本体变成 digest 锁定的制品，声明就从「同名」变成「同字节」。

判分口径（抽取按各 benchmark 官方约定，等价性判定一律交给公开库）：

| dataset | 抽取（benchmark 官方约定） | 等价判定（公开库） |
|---|---|---|
| gsm8k | 最后一个 `####` 之后的内容，无标记则取全文 | `math_verify.verify`（`verifiers.MathRubric` 内核） |
| olymmath[-easy/-hard] | `verifiers.extract_boxed_answer`，无 boxed 回退 `####` | 同上 |
| pubmedqa | 官方三分类 yes/no/maybe，取最后出现的整词 | `verifiers.Rubric` 上的 label_match reward func |
| scitab | 官方三分类 supports/refutes/not enough info，取最后出现的整词 | 同上 |

用法：

    from qa_rubric import score, build_rubric, DATASETS
    reward = score("gsm8k", completion_text, ground_truth)

依赖：`verifiers`、`math-verify`（内网侧由 EnvPackage 的 wheelhouse 提供）。
"""

from __future__ import annotations

import asyncio
import re
from typing import Any

__all__ = [
    "DATASETS",
    "PUBMEDQA_LABELS",
    "SCITAB_LABELS",
    "ReferenceScorer",
    "build_rubric",
    "extract",
    "extract_gsm8k",
    "extract_olymmath",
    "extract_pubmedqa",
    "extract_scitab",
    "score",
]

#: 本规则包覆盖的 dataset 路由键，与 `plugins/qa/manifest.yaml` 及
#: `plugins/math/src/score.rs` 的路由保持一致。声明在这里是为了让
#: 「规则包覆盖哪些 dataset」可被机器核对，而不是靠读代码。
DATASETS = (
    "gsm8k",
    "pubmedqa",
    "scitab",
    "olymmath",
    "olymmath-easy",
    "olymmath-hard",
)

PUBMEDQA_LABELS = ("yes", "no", "maybe")
SCITAB_LABELS = ("supports", "refutes", "not enough info")
# 官方标签的常见同义写法；归一到官方标签后再比较。
SCITAB_ALIASES = {
    "not enough information": "not enough info",
    "notenoughinfo": "not enough info",
    "support": "supports",
    "refute": "refutes",
}


# ---------------------------------------------------------------------------
# 抽取器（benchmark 官方约定）
# ---------------------------------------------------------------------------
def extract_gsm8k(text: str) -> str:
    """GSM8K 官方约定：`#### <answer>`；无标记时退回全文。"""
    if "####" in text:
        return text.rsplit("####", 1)[1].strip()
    return text.strip()


def extract_olymmath(text: str) -> str:
    """OlymMATH 约定：最后一个 `\\boxed{...}`；无 boxed 时回退 `####`，再回退全文。

    boxed 抽取直接用 `verifiers` 的公开实现，避免自研 parser。
    """
    from verifiers import extract_boxed_answer

    boxed = extract_boxed_answer(text, strict=False)
    if boxed:
        return boxed.strip()
    return extract_gsm8k(text)


def _last_label(text: str, labels: tuple[str, ...]) -> str | None:
    """取最后出现的整词标签（多词标签允许内部空白）。"""
    lowered = text.lower()
    best: tuple[int, str] | None = None
    for label in labels:
        pattern = r"\b" + r"\s+".join(re.escape(part) for part in label.split()) + r"\b"
        for match in re.finditer(pattern, lowered):
            if best is None or match.start() > best[0]:
                best = (match.start(), label)
    return best[1] if best else None


def extract_pubmedqa(text: str) -> str | None:
    return _last_label(text, PUBMEDQA_LABELS)


def extract_scitab(text: str) -> str | None:
    candidates = SCITAB_LABELS + tuple(SCITAB_ALIASES)
    label = _last_label(text, candidates)
    if label is None:
        return None
    return SCITAB_ALIASES.get(label, label)


def extract(dataset: str, text: str) -> str | None:
    """按 dataset 官方约定抽取最终答案。"""
    if dataset == "gsm8k":
        return extract_gsm8k(text)
    if dataset.startswith("olymmath"):
        return extract_olymmath(text)
    if dataset == "pubmedqa":
        return extract_pubmedqa(text)
    if dataset == "scitab":
        return extract_scitab(text)
    raise ValueError(f"unsupported dataset for reference extraction: {dataset}")


# ---------------------------------------------------------------------------
# 参照判分：数学走 verifiers.MathRubric，分类走 verifiers.Rubric 上的 label_match
# ---------------------------------------------------------------------------
class ReferenceScorer:
    """公开库上的金标判分器。

    数学类等价性判定完全委托 `verifiers.MathRubric`（内核为 `math_verify`）；
    分类类在公开 `Rubric` 上注册 label_match reward func，保持「判分函数注册在
    公开框架里」的形态，而不是自研一套比较逻辑。
    """

    def __init__(self) -> None:
        from verifiers import Rubric
        from verifiers.rubrics.math_rubric import MathRubric

        self.math_rubric = MathRubric()
        # 分类任务：verifiers 没有内置三分类 Rubric，这里在公开 Rubric 上挂
        # 官方标签匹配 reward func。
        self.label_rubric = Rubric()
        self.label_rubric.add_reward_func(self._label_match)
        self._label_ctx: dict[str, Any] = {}

    async def _label_match(self, completion: Any, answer: str, **_kwargs: Any) -> float:
        dataset = self._label_ctx.get("dataset", "")
        text = self._label_ctx.get("text", "")
        extractor = extract_pubmedqa if dataset == "pubmedqa" else extract_scitab
        predicted = extractor(text)
        expected = extractor(answer) or answer.strip().lower()
        return 1.0 if predicted is not None and predicted == expected else 0.0

    def score_math(self, action: str, target: str, dataset: str) -> float:
        extracted = extract_gsm8k(action) if dataset == "gsm8k" else extract_olymmath(action)
        if not extracted:
            return 0.0
        # 把抽取结果包成 boxed，交给 MathRubric 默认 parser + math_verify 判等价，
        # 这样等价性判定完全由公开库负责，我们只负责按官方约定抽取。
        completion = [{"role": "assistant", "content": f"\\boxed{{{extracted}}}"}]
        return float(
            asyncio.run(
                self.math_rubric.correct_answer(
                    parser=self.math_rubric.parser,
                    completion=completion,
                    answer=target,
                )
            )
        )

    def score_label(self, action: str, target: str, dataset: str) -> float:
        self._label_ctx = {"dataset": dataset, "text": action}
        completion = [{"role": "assistant", "content": action}]
        return float(asyncio.run(self._label_match(completion=completion, answer=target)))

    def score(self, dataset: str, action: str, target: str) -> float:
        if dataset == "gsm8k" or dataset.startswith("olymmath"):
            return self.score_math(action, target, "gsm8k" if dataset == "gsm8k" else "olymmath")
        if dataset in ("pubmedqa", "scitab"):
            return self.score_label(action, target, dataset)
        raise ValueError(f"unsupported dataset for reference scoring: {dataset}")


def build_rubric(dataset: str) -> Any:
    """返回该 dataset 对应的 verifiers Rubric 对象。

    数学类返回 `MathRubric`，分类类返回挂了 label_match 的 `Rubric`。供需要直接
    拿 verifiers 对象（而不是只要一个 float）的调用方使用。
    """
    if dataset not in DATASETS:
        raise ValueError(f"unsupported dataset: {dataset}")
    scorer = ReferenceScorer()
    if dataset == "gsm8k" or dataset.startswith("olymmath"):
        return scorer.math_rubric
    return scorer.label_rubric


def score(dataset: str, action: str, target: str) -> float:
    """金标判分入口：`RubricSpec.reference_scorer.entrypoint` 指向这里。"""
    return ReferenceScorer().score(dataset, action, target)
