//! 金标对齐取数器：用**生产判分入口** `score_action` 给对齐语料打分。
//!
//! 输入 JSONL，每行 `{"case_id", "dataset", "action", "target"}`；
//! 输出 JSONL，每行追加 `"uenv_reward"`（1.0 / 0.0）。
//!
//! 之所以走 example 而不是重写一份 Python 版：对齐要证明的是**线上这段 Rust 判分**
//! 与公开 verifiers Rubric 一致，任何重写都会让结论失效。
//!
//! 用法：
//!   cargo run -p uenv-math-env --example score_corpus -- corpus.jsonl > uenv_scores.jsonl
//!   cat corpus.jsonl | cargo run -p uenv-math-env --example score_corpus > uenv_scores.jsonl

use std::env;
use std::fs;
use std::io::{self, Read, Write};

use serde_json::Value;
use uenv_math_env::score::score_action;

fn main() -> io::Result<()> {
    let mut raw = String::new();
    match env::args().nth(1) {
        Some(path) if path != "-" => raw = fs::read_to_string(path)?,
        _ => {
            io::stdin().read_to_string(&mut raw)?;
        }
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    for (idx, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut row: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {}: invalid json: {e}", idx + 1));
        let dataset = row
            .get("dataset")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let action = row
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let target = row
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let reward = score_action(&dataset, &action, &target);
        row.as_object_mut()
            .expect("corpus rows must be json objects")
            .insert("uenv_reward".into(), Value::from(reward));
        writeln!(out, "{}", serde_json::to_string(&row).expect("serialize"))?;
    }
    Ok(())
}
