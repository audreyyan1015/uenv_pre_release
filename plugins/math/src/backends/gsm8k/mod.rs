//! GSM8K benchmark backend（对齐 VeRL `extract_solution`）。

mod scoring;

pub use scoring::{answers_match, extract_solution, normalize_answer};
