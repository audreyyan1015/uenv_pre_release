//! OpenEnv → UEnv standardized-environment conversion.
//!
//! Upstream reference: HuggingFace / meta-pytorch **OpenEnv** (RFC 002 "env spec"),
//! where an environment ships as
//!
//! ```text
//! my_env/
//! ├── openenv.yaml      # manifest
//! ├── models.py         # Action / Observation / State (pydantic)
//! ├── client.py
//! ├── server/app.py     # FastAPI `create_app(...)`
//! └── Dockerfile
//! ```
//!
//! and exposes the Gymnasium-style baseline API `reset()` / `step(action)` /
//! `state()` over HTTP (plus `/health`).
//!
//! This module converts such a project into our authoritative declaration
//! (`manifest.toml`, see `Docs/hub/260716-标准化环境定义规范.md`) by
//!
//! 1. parsing `openenv.yaml` (a **strict subset** of YAML — see [`parse_yaml`]),
//! 2. deriving the OpenEnv `InterfaceSchema` (Action/Observation/State JSON
//!    Schemas) from the pydantic classes in `models.py`, injecting the fields the
//!    OpenEnv base classes are specified to carry (`Observation.done/reward/
//!    metadata`, `State.episode_id/step_count`),
//! 3. rewriting the runtime image to an intranet-reachable reference so the
//!    converted environment satisfies 内网零外拉 (zero egress) by construction.
//!
//! Real-world `openenv.yaml` files vary in shape (observed on the `openenv` HF
//! org): some are deployment-oriented (`runtime`/`app`/`port`), some declare the
//! contract classes as bare scalars (`action: CodeAction`), and the documented
//! form uses nested `{class_name, module}` maps. All three are accepted.

use crate::domain::manifest::public_registry_of;
use serde_json::{Map as JsonMap, Value};
use std::collections::BTreeMap;
use uenv_hub_types::{InterfaceSchema, ValidationReport};

/// Converter revision, recorded in the generated manifest for auditability.
pub const CONVERTER_VERSION: &str = "openenv-import/1";

/// Errors that abort a conversion (as opposed to findings, which are collected
/// into a [`ValidationReport`]).
#[derive(Debug, thiserror::Error)]
pub enum OpenEnvError {
    /// A YAML construct outside the supported subset (anchors, block scalars,
    /// tab indentation, multiple documents…). Reported with a line number so the
    /// author can fix the source rather than guess.
    #[error("openenv.yaml line {line}: unsupported YAML construct ({detail})")]
    UnsupportedYaml { line: usize, detail: String },
    #[error("openenv.yaml line {line}: malformed entry ({detail})")]
    Malformed { line: usize, detail: String },
    #[error("openenv.yaml is missing required field '{0}'")]
    MissingField(&'static str),
}

// ---------------------------------------------------------------------------
// YAML subset
// ---------------------------------------------------------------------------

/// A node of the supported YAML subset: scalars, nested maps and scalar lists.
#[derive(Debug, Clone, PartialEq)]
pub enum YamlNode {
    Scalar(String),
    Map(BTreeMap<String, YamlNode>),
    List(Vec<String>),
}

impl YamlNode {
    fn as_scalar(&self) -> Option<&str> {
        match self {
            YamlNode::Scalar(s) => Some(s.as_str()),
            _ => None,
        }
    }
    fn get(&self, key: &str) -> Option<&YamlNode> {
        match self {
            YamlNode::Map(m) => m.get(key),
            _ => None,
        }
    }

    /// Child node under `key`, whatever its shape.
    pub fn get_map(&self, key: &str) -> Option<&YamlNode> {
        self.get(key)
    }

    /// Child scalar under `key`, empty scalars treated as absent.
    pub fn get_scalar(&self, key: &str) -> Option<&str> {
        self.get(key)
            .and_then(YamlNode::as_scalar)
            .filter(|s| !s.trim().is_empty())
    }
}

struct RawLine {
    no: usize,
    indent: usize,
    text: String,
}

/// Strip a trailing `#` comment, honouring quoted spans so `a: "x # y"` keeps
/// its value intact.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'#' => {
                    // A comment marker only counts at line start or after space.
                    if i == 0 || bytes[i - 1].is_ascii_whitespace() {
                        return &line[..i];
                    }
                }
                _ => {}
            },
        }
    }
    line
}

fn unquote(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2 {
        let b = t.as_bytes();
        if (b[0] == b'"' && b[t.len() - 1] == b'"') || (b[0] == b'\'' && b[t.len() - 1] == b'\'') {
            return t[1..t.len() - 1].to_string();
        }
    }
    t.to_string()
}

fn lex(src: &str) -> Result<Vec<RawLine>, OpenEnvError> {
    let mut out = Vec::new();
    let mut docs = 0usize;
    for (i, raw) in src.lines().enumerate() {
        let no = i + 1;
        let content = strip_comment(raw);
        let trimmed_start = content.trim_start();
        if trimmed_start.is_empty() {
            continue;
        }
        let indent_len = content.len() - trimmed_start.len();
        if content[..indent_len].contains('\t') {
            return Err(OpenEnvError::UnsupportedYaml {
                line: no,
                detail: "tab character in indentation; use spaces".into(),
            });
        }
        let text = trimmed_start.trim_end().to_string();
        if text == "---" {
            docs += 1;
            if docs > 1 {
                return Err(OpenEnvError::UnsupportedYaml {
                    line: no,
                    detail: "multi-document YAML is not supported".into(),
                });
            }
            continue;
        }
        if text == "..." {
            continue;
        }
        if text.starts_with('&') || text.starts_with('*') || text.starts_with("<<") {
            return Err(OpenEnvError::UnsupportedYaml {
                line: no,
                detail: "anchors/aliases/merge keys are not supported".into(),
            });
        }
        if text.ends_with('|') || text.ends_with('>') {
            return Err(OpenEnvError::UnsupportedYaml {
                line: no,
                detail: "block scalars ('|', '>') are not supported".into(),
            });
        }
        out.push(RawLine {
            no,
            indent: indent_len,
            text,
        });
    }
    Ok(out)
}

fn parse_map(lines: &[RawLine], pos: &mut usize, indent: usize) -> Result<YamlNode, OpenEnvError> {
    let mut map = BTreeMap::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(OpenEnvError::Malformed {
                line: line.no,
                detail: "unexpected indentation".into(),
            });
        }
        if line.text.starts_with("- ") || line.text == "-" {
            break;
        }
        let Some(colon) = line.text.find(':') else {
            return Err(OpenEnvError::Malformed {
                line: line.no,
                detail: "expected 'key: value'".into(),
            });
        };
        let key = line.text[..colon].trim().to_string();
        let rest = line.text[colon + 1..].trim().to_string();
        // Anchors/aliases may also appear in value position (`base: &a 1`).
        if rest.starts_with('&') || rest.starts_with('*') {
            return Err(OpenEnvError::UnsupportedYaml {
                line: line.no,
                detail: "anchors/aliases are not supported".into(),
            });
        }
        *pos += 1;
        if !rest.is_empty() {
            map.insert(key, YamlNode::Scalar(unquote(&rest)));
            continue;
        }
        // Empty value → nested block (map or list) if more-indented lines follow.
        let child_indent = lines.get(*pos).map(|l| l.indent).unwrap_or(0);
        if *pos < lines.len() && child_indent > indent {
            if lines[*pos].text.starts_with("- ") {
                let mut items = Vec::new();
                while *pos < lines.len()
                    && lines[*pos].indent == child_indent
                    && lines[*pos].text.starts_with("- ")
                {
                    items.push(unquote(&lines[*pos].text[2..]));
                    *pos += 1;
                }
                map.insert(key, YamlNode::List(items));
            } else {
                let child = parse_map(lines, pos, child_indent)?;
                map.insert(key, child);
            }
        } else {
            map.insert(key, YamlNode::Scalar(String::new()));
        }
    }
    Ok(YamlNode::Map(map))
}

/// Parse the supported YAML subset. Unsupported constructs fail loudly (with a
/// line number) instead of being silently mis-read — the conversion of an
/// environment contract must never guess.
pub fn parse_yaml(src: &str) -> Result<YamlNode, OpenEnvError> {
    let lines = lex(src)?;
    let mut pos = 0usize;
    let base = lines.first().map(|l| l.indent).unwrap_or(0);
    let node = parse_map(&lines, &mut pos, base)?;
    if pos < lines.len() {
        return Err(OpenEnvError::Malformed {
            line: lines[pos].no,
            detail: "trailing content outside the top-level mapping".into(),
        });
    }
    Ok(node)
}

// ---------------------------------------------------------------------------
// openenv.yaml model
// ---------------------------------------------------------------------------

/// Reference to a contract class, e.g. `action: CodeAction` or
/// `action: {class_name: MyAction, module: my_env.models}`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassRef {
    pub class_name: String,
    pub module: Option<String>,
}

/// The parsed `openenv.yaml`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenEnvSpec {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub spec_version: Option<String>,
    pub default_image: Option<String>,
    pub action: Option<ClassRef>,
    pub observation: Option<ClassRef>,
    pub state: Option<ClassRef>,
    pub client: Option<ClassRef>,
    /// Deployment hints seen in real spaces: `runtime: fastapi`, `app:
    /// server.app:app`, `port: 8000`, `type: space`.
    pub runtime: Option<String>,
    pub app: Option<String>,
    pub port: Option<u32>,
    pub kind: Option<String>,
}

fn class_ref(node: Option<&YamlNode>) -> Option<ClassRef> {
    match node? {
        YamlNode::Scalar(s) if !s.trim().is_empty() => Some(ClassRef {
            class_name: s.trim().to_string(),
            module: None,
        }),
        YamlNode::Map(m) => {
            let class_name = m
                .get("class_name")
                .and_then(|v| v.as_scalar())
                .unwrap_or_default()
                .to_string();
            if class_name.is_empty() {
                return None;
            }
            Some(ClassRef {
                class_name,
                module: m
                    .get("module")
                    .and_then(|v| v.as_scalar())
                    .map(|s| s.to_string()),
            })
        }
        _ => None,
    }
}

impl OpenEnvSpec {
    /// Read an `openenv.yaml` document.
    pub fn from_yaml(src: &str) -> Result<Self, OpenEnvError> {
        let root = parse_yaml(src)?;
        let name = root
            .get("name")
            .and_then(|v| v.as_scalar())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or(OpenEnvError::MissingField("name"))?;
        let scalar = |k: &str| {
            root.get(k)
                .and_then(|v| v.as_scalar())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        Ok(Self {
            name,
            version: scalar("version"),
            description: scalar("description"),
            spec_version: scalar("spec_version"),
            default_image: scalar("default_image").or_else(|| scalar("image")),
            action: class_ref(root.get("action")),
            observation: class_ref(root.get("observation")),
            state: class_ref(root.get("state")),
            client: class_ref(root.get("client")),
            runtime: scalar("runtime"),
            app: scalar("app"),
            port: scalar("port").and_then(|p| p.parse().ok()),
            kind: scalar("type"),
        })
    }
}

// ---------------------------------------------------------------------------
// models.py → JSON Schema
// ---------------------------------------------------------------------------

/// One field of a pydantic contract class.
#[derive(Debug, Clone, PartialEq)]
pub struct PyField {
    pub name: String,
    pub schema: Value,
    pub required: bool,
    pub description: Option<String>,
}

/// A class declared in `models.py`.
#[derive(Debug, Clone, PartialEq)]
pub struct PyClass {
    pub name: String,
    pub bases: Vec<String>,
    pub fields: Vec<PyField>,
}

/// Map a Python type annotation onto a JSON Schema fragment. Unknown
/// annotations map to the permissive `{}` (any) rather than a wrong type.
pub fn py_type_to_schema(ty: &str) -> Value {
    let t = ty.trim();
    let lower = t.to_ascii_lowercase();
    // Optional[X] / X | None → nullable X
    let inner_opt = if lower.starts_with("optional[") && t.ends_with(']') {
        Some(t["optional[".len()..t.len() - 1].to_string())
    } else if let Some(stripped) = t.strip_suffix("| None").map(|s| s.trim().to_string()) {
        Some(stripped)
    } else {
        None
    };
    if let Some(inner) = inner_opt {
        let base = py_type_to_schema(&inner);
        return nullable(base);
    }
    if lower.starts_with("list[") && t.ends_with(']') {
        let inner = &t["list[".len()..t.len() - 1];
        return serde_json::json!({"type": "array", "items": py_type_to_schema(inner)});
    }
    if lower.starts_with("dict[") || lower == "dict" {
        return serde_json::json!({"type": "object"});
    }
    if lower.starts_with("union[") && t.ends_with(']') {
        let inner = &t["union[".len()..t.len() - 1];
        let mut types: Vec<Value> = Vec::new();
        for part in split_top_level(inner) {
            if let Some(Value::String(s)) = py_type_to_schema(&part).get("type").cloned() {
                if !types.iter().any(|v| v == &Value::String(s.clone())) {
                    types.push(Value::String(s));
                }
            } else if part.trim() == "None" {
                types.push(Value::String("null".into()));
            }
        }
        if types.is_empty() {
            return serde_json::json!({});
        }
        if types.len() == 1 {
            return serde_json::json!({"type": types[0].clone()});
        }
        return serde_json::json!({"type": Value::Array(types)});
    }
    match lower.as_str() {
        "str" => serde_json::json!({"type": "string"}),
        "int" => serde_json::json!({"type": "integer"}),
        "float" => serde_json::json!({"type": "number"}),
        "bool" => serde_json::json!({"type": "boolean"}),
        "list" => serde_json::json!({"type": "array"}),
        _ => serde_json::json!({}),
    }
}

fn nullable(base: Value) -> Value {
    match base.get("type") {
        Some(Value::String(s)) => {
            let mut out = base.clone();
            out["type"] = Value::Array(vec![
                Value::String(s.clone()),
                Value::String("null".into()),
            ]);
            out
        }
        _ => base,
    }
}

/// Split `a, b[c, d], e` on top-level commas only.
fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '[' | '(' => {
                depth += 1;
                cur.push(ch);
            }
            ']' | ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn extract_kwarg(call: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let idx = call.find(&needle)?;
    let rest = call[idx + needle.len()..].trim_start();
    let mut chars = rest.chars();
    let first = chars.next()?;
    if first == '"' || first == '\'' {
        let end = rest[1..].find(first)? + 1;
        return Some(rest[1..end].to_string());
    }
    let end = rest
        .find([',', ')'])
        .unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// Parse the contract classes out of a `models.py`. This is a deliberately
/// narrow reader for the pydantic style OpenEnv prescribes (`class X(Action):`
/// with annotated fields); anything it cannot interpret is simply skipped, which
/// surfaces later as a contract-completeness finding rather than a wrong schema.
pub fn parse_python_models(src: &str) -> Vec<PyClass> {
    let mut classes: Vec<PyClass> = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if !trimmed.starts_with("class ") {
            i += 1;
            continue;
        }
        let class_indent = line.len() - trimmed.len();
        let header = trimmed["class ".len()..].trim();
        let (name, bases) = match header.find('(') {
            Some(p) => {
                let name = header[..p].trim().to_string();
                let close = header.rfind(')').unwrap_or(header.len());
                let bases_raw = &header[p + 1..close.max(p + 1)];
                let bases = split_top_level(bases_raw)
                    .into_iter()
                    // `Environment[ActT, ObsT]` → keep the head identifier
                    .map(|b| {
                        b.split('[')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .trim_end_matches(':')
                            .to_string()
                    })
                    .filter(|b| !b.is_empty())
                    .collect();
                (name, bases)
            }
            None => (
                header.trim_end_matches(':').trim().to_string(),
                Vec::new(),
            ),
        };
        i += 1;
        let mut fields = Vec::new();
        while i < lines.len() {
            let body = lines[i];
            let body_trim = body.trim_start();
            if body_trim.is_empty() {
                i += 1;
                continue;
            }
            let indent = body.len() - body_trim.len();
            if indent <= class_indent {
                break; // class body ended
            }
            // Skip docstrings.
            if body_trim.starts_with("\"\"\"") || body_trim.starts_with("'''") {
                let quote = &body_trim[..3];
                let single_line = body_trim.len() >= 6 && body_trim[3..].contains(quote);
                i += 1;
                if !single_line {
                    while i < lines.len() && !lines[i].contains(quote) {
                        i += 1;
                    }
                    i += 1;
                }
                continue;
            }
            if body_trim.starts_with('#')
                || body_trim.starts_with('@')
                || body_trim.starts_with("def ")
                || body_trim.starts_with("class ")
                || body_trim.starts_with("pass")
                || body_trim.starts_with("return")
            {
                i += 1;
                continue;
            }
            // Join continuation lines so multi-line `Field(...)` calls parse.
            let mut stmt = body_trim.to_string();
            while stmt.matches('(').count() > stmt.matches(')').count() && i + 1 < lines.len() {
                i += 1;
                stmt.push(' ');
                stmt.push_str(lines[i].trim());
            }
            i += 1;
            let Some(colon) = stmt.find(':') else { continue };
            let fname = stmt[..colon].trim();
            if fname.is_empty() || !is_identifier(fname) {
                continue;
            }
            let after = stmt[colon + 1..].trim();
            let (ty, default) = match after.find('=') {
                Some(eq) => (after[..eq].trim(), Some(after[eq + 1..].trim())),
                None => (after, None),
            };
            if ty.is_empty() {
                continue;
            }
            let mut schema = py_type_to_schema(ty);
            let mut description = None;
            let mut required = default.is_none();
            if let Some(def) = default {
                if def.starts_with("Field(") {
                    // `Field(...)` (Ellipsis) marks a required field.
                    let inner = &def["Field(".len()..def.len().saturating_sub(1)];
                    let first = split_top_level(inner).into_iter().next().unwrap_or_default();
                    required = first.trim() == "...";
                    description = extract_kwarg(def, "description");
                    if let Some(d) = extract_kwarg(def, "default") {
                        if d == "None" {
                            schema = nullable(schema);
                        }
                    }
                } else if def == "None" {
                    schema = nullable(schema);
                }
            }
            if let (Some(desc), Value::Object(map)) = (description.clone(), &mut schema) {
                map.insert("description".into(), Value::String(desc));
            }
            fields.push(PyField {
                name: fname.to_string(),
                schema,
                required,
                description,
            });
        }
        classes.push(PyClass {
            name,
            bases,
            fields,
        });
    }
    classes
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// Fields the OpenEnv **base** classes are specified to carry (RFC 002 for
/// `Observation`, the concepts guide for `State`). They are injected so the
/// generated contract matches what the server actually serialises, not just the
/// subclass' own additions.
fn base_fields(kind: Kind) -> Vec<(&'static str, Value, bool)> {
    match kind {
        Kind::Observation => vec![
            (
                "done",
                serde_json::json!({"type": "boolean", "description": "episode terminated (OpenEnv Observation base)"}),
                false,
            ),
            (
                "reward",
                serde_json::json!({"type": ["number", "boolean", "null"], "description": "environment-computed reward (OpenEnv Observation base)"}),
                false,
            ),
            (
                "metadata",
                serde_json::json!({"type": "object", "description": "free-form metadata (OpenEnv Observation base)"}),
                false,
            ),
        ],
        Kind::State => vec![
            (
                "episode_id",
                serde_json::json!({"type": "string", "description": "current episode id (OpenEnv State base)"}),
                false,
            ),
            (
                "step_count",
                serde_json::json!({"type": "integer", "description": "steps taken in this episode (OpenEnv State base)"}),
                false,
            ),
        ],
        Kind::Action => Vec::new(),
    }
}

/// Which side of the contract a class implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Action,
    Observation,
    State,
}

impl Kind {
    fn base_name(self) -> &'static str {
        match self {
            Kind::Action => "Action",
            Kind::Observation => "Observation",
            Kind::State => "State",
        }
    }
}

/// Build the JSON Schema for one contract side from the parsed classes.
///
/// Selection order: the class named in `openenv.yaml`, else the first class
/// whose base list contains the OpenEnv base (`Action`/`Observation`/`State`).
pub fn schema_for(classes: &[PyClass], declared: Option<&ClassRef>, kind: Kind) -> Option<Value> {
    let class = declared
        .and_then(|r| classes.iter().find(|c| c.name == r.class_name))
        .or_else(|| {
            classes
                .iter()
                .find(|c| c.bases.iter().any(|b| b == kind.base_name()))
        })?;

    let mut props = JsonMap::new();
    let mut required: Vec<Value> = Vec::new();
    for (name, schema, req) in base_fields(kind) {
        props.insert(name.to_string(), schema);
        if req {
            required.push(Value::String(name.to_string()));
        }
    }
    for f in &class.fields {
        props.insert(f.name.clone(), f.schema.clone());
        if f.required {
            required.push(Value::String(f.name.clone()));
        }
    }
    let mut out = JsonMap::new();
    out.insert("type".into(), Value::String("object".into()));
    out.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        out.insert("required".into(), Value::Array(required));
    }
    Some(Value::Object(out))
}

// ---------------------------------------------------------------------------
// manifest.toml emission
// ---------------------------------------------------------------------------

fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    format!("\"{out}\"")
}

fn toml_inline(v: &Value) -> Option<String> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if n.is_f64() {
                let f = n.as_f64().unwrap_or_default();
                if f.fract() == 0.0 {
                    format!("{f:.1}")
                } else {
                    f.to_string()
                }
            } else {
                n.to_string()
            }
        }
        Value::String(s) => toml_escape(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(toml_inline).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .filter_map(|(k, v)| toml_inline(v).map(|s| format!("{} = {}", toml_key(k), s)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    })
}

fn toml_key(k: &str) -> String {
    let bare = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        k.to_string()
    } else {
        toml_escape(k)
    }
}

/// Emit a JSON object as TOML tables under `path`, writing **scalars before
/// sub-tables** (a TOML requirement) and recursing deterministically.
pub(crate) fn emit_table(path: &str, obj: &JsonMap<String, Value>, out: &mut String) {
    if !path.is_empty() {
        out.push_str(&format!("[{path}]\n"));
    }
    for (k, v) in obj {
        if matches!(v, Value::Object(_)) || matches!(v, Value::Null) {
            continue;
        }
        if let Some(lit) = toml_inline(v) {
            out.push_str(&format!("{} = {}\n", toml_key(k), lit));
        }
    }
    for (k, v) in obj {
        if let Value::Object(child) = v {
            let child_path = if path.is_empty() {
                toml_key(k)
            } else {
                format!("{path}.{}", toml_key(k))
            };
            out.push('\n');
            emit_table(&child_path, child, out);
        }
    }
}

// ---------------------------------------------------------------------------
// conversion
// ---------------------------------------------------------------------------

/// Knobs for a conversion run.
#[derive(Debug, Clone)]
pub struct ConvertOptions {
    /// Intranet registry prefix used to rewrite the runtime image, e.g.
    /// `registry.uenv.internal/openenv`. When set, the emitted `[image].url` is
    /// always intranet-reachable (零外拉 by construction).
    pub internal_registry: Option<String>,
    pub namespace: Option<String>,
    pub author: Option<String>,
    /// Override the derived `env_type`.
    pub env_type: Option<String>,
    /// Version stated by the operator. Wins over anything found in the source,
    /// and — unlike [`Self::fallback_version`] — is not reported as a guess.
    pub explicit_version: Option<String>,
    /// Version used when neither the source nor the operator states one.
    pub fallback_version: String,
    /// Relative path of the dependency manifest found in the project
    /// (`requirements.txt`), emitted as `[dependencies].requirements_path` so the
    /// offline wheelhouse check has something to compare against.
    pub requirements_path: Option<String>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            internal_registry: None,
            namespace: None,
            author: None,
            env_type: None,
            explicit_version: None,
            fallback_version: "0.1.0".into(),
            requirements_path: None,
        }
    }
}

/// Result of a conversion.
#[derive(Debug, Clone)]
pub struct Converted {
    pub env_type: String,
    pub version: String,
    pub interface: InterfaceSchema,
    /// The generated `manifest.toml` (authoritative declaration).
    pub manifest_toml: String,
    /// Findings: zero-egress rewrites, missing contract sides, guessed values.
    pub report: ValidationReport,
    /// Human-readable provenance of every non-trivial mapping decision.
    pub notes: Vec<String>,
}

/// Normalise an OpenEnv `name` into a legal `env_type`
/// (lowercase, `[a-z0-9._-]`).
pub fn sanitize_env_type(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Convert a parsed OpenEnv project into our `manifest.toml` + contract.
///
/// `models_src` is the text of `models.py` when available; without it the
/// contract cannot be derived and the report says so explicitly (we never
/// fabricate a contract).
pub fn convert(
    spec: &OpenEnvSpec,
    models_src: Option<&str>,
    opts: &ConvertOptions,
) -> Result<Converted, OpenEnvError> {
    let mut report = ValidationReport::ok();
    let mut notes: Vec<String> = Vec::new();

    let env_type = opts
        .env_type
        .clone()
        .unwrap_or_else(|| sanitize_env_type(&spec.name));
    if env_type.is_empty() {
        return Err(OpenEnvError::MissingField("name"));
    }
    if env_type != spec.name {
        notes.push(format!(
            "env_type '{}' derived from openenv.yaml name '{}'",
            env_type, spec.name
        ));
    }

    let version = match opts.explicit_version.clone().or_else(|| spec.version.clone()) {
        Some(v) => {
            if let (Some(explicit), Some(declared)) = (&opts.explicit_version, &spec.version) {
                if explicit != declared {
                    notes.push(format!(
                        "version '{explicit}' given on the command line overrides openenv.yaml                          version '{declared}'"
                    ));
                }
            }
            v
        }
        None => {
            report.push_warning(
                "version",
                format!(
                    "openenv.yaml declares no 'version'; defaulted to {} — set it explicitly before publishing",
                    opts.fallback_version
                ),
            );
            opts.fallback_version.clone()
        }
    };

    // ---- contract ----
    let classes = models_src.map(parse_python_models).unwrap_or_default();
    if models_src.is_none() {
        report.push_warning(
            "interface",
            "models.py not provided; the OpenEnv Action/Observation/State contract could not be derived",
        );
    }
    let interface = InterfaceSchema {
        action: schema_for(&classes, spec.action.as_ref(), Kind::Action),
        observation: schema_for(&classes, spec.observation.as_ref(), Kind::Observation),
        state: schema_for(&classes, spec.state.as_ref(), Kind::State),
    };
    for (label, present) in [
        ("action", interface.action.is_some()),
        ("observation", interface.observation.is_some()),
        ("state", interface.state.is_some()),
    ] {
        if !present {
            report.push_warning(
                format!("interface.{label}"),
                format!(
                    "no `{}` class found in models.py (declare it in openenv.yaml or subclass the OpenEnv base)",
                    label
                ),
            );
        }
    }
    if !classes.is_empty() {
        notes.push(format!(
            "contract derived from models.py classes: {}",
            classes
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // ---- runtime image (zero egress) ----
    let mut image_url: Option<String> = None;
    match (&spec.default_image, &opts.internal_registry) {
        (Some(orig), Some(reg)) => {
            let internal = format!("{}/{}:{}", reg.trim_end_matches('/'), env_type, version);
            if orig != &internal {
                notes.push(format!(
                    "runtime image rewritten for 内网零外拉: '{orig}' -> '{internal}'"
                ));
            }
            image_url = Some(internal);
        }
        (Some(orig), None) => {
            if let Some(reg) = public_registry_of(orig) {
                report.push_warning(
                    "image.url",
                    format!(
                        "openenv.yaml default_image references public registry '{reg}'; re-run with \
                         --registry <internal-host> so the converted manifest is intranet-only"
                    ),
                );
            }
            image_url = Some(orig.clone());
        }
        (None, Some(reg)) => {
            let internal = format!("{}/{}:{}", reg.trim_end_matches('/'), env_type, version);
            notes.push(format!(
                "openenv.yaml declares no default_image; intranet reference '{internal}' generated"
            ));
            image_url = Some(internal);
        }
        (None, None) => {
            report.push_warning(
                "image",
                "openenv.yaml declares no default_image and no --registry was given; \
                 add [image].url (intranet-reachable) before publishing",
            );
        }
    }

    // ---- entrypoint derived from the FastAPI deployment hints ----
    let port = spec.port.unwrap_or(8000);
    let entrypoint = match &spec.app {
        Some(app) => {
            let ep = format!("uvicorn {app} --host 0.0.0.0 --port {port}");
            notes.push(format!(
                "entrypoint derived from openenv.yaml app='{app}' port={port}"
            ));
            Some(ep)
        }
        None => {
            notes.push(
                "openenv.yaml declares no 'app'; entrypoint left to the image CMD (OpenEnv Dockerfile)"
                    .into(),
            );
            None
        }
    };

    // ---- assemble the manifest ----
    let mut root = JsonMap::new();
    root.insert("env_type".into(), Value::String(env_type.clone()));
    root.insert(
        "namespace".into(),
        Value::String(opts.namespace.clone().unwrap_or_else(|| "default".into())),
    );
    root.insert(
        "description".into(),
        Value::String(spec.description.clone().unwrap_or_else(|| {
            format!("Imported from OpenEnv environment '{}'.", spec.name)
        })),
    );
    if let Some(author) = &opts.author {
        root.insert("author".into(), Value::String(author.clone()));
    }
    let mut tags = vec![Value::String("openenv".into())];
    if let Some(sv) = &spec.spec_version {
        tags.push(Value::String(format!("openenv-spec-{sv}")));
    }
    root.insert("tags".into(), Value::Array(tags));

    let mut version_tbl = JsonMap::new();
    version_tbl.insert("version".into(), Value::String(version.clone()));
    version_tbl.insert(
        "changelog".into(),
        Value::String(format!(
            "Imported from OpenEnv '{}' via {}.",
            spec.name, CONVERTER_VERSION
        )),
    );
    if let Some(ep) = &entrypoint {
        version_tbl.insert("entrypoint".into(), Value::String(ep.clone()));
    }
    version_tbl.insert(
        "supported_backends".into(),
        Value::Array(vec![
            Value::String("docker".into()),
            Value::String("podman".into()),
        ]),
    );
    // OpenEnv servers expose /health (RFC 002).
    version_tbl.insert("health_check_path".into(), Value::String("/health".into()));
    root.insert("version".into(), Value::Object(version_tbl));

    if let Some(url) = &image_url {
        let mut image_tbl = JsonMap::new();
        image_tbl.insert("url".into(), Value::String(url.clone()));
        image_tbl.insert("arch".into(), Value::String("amd64".into()));
        root.insert("image".into(), Value::Object(image_tbl));
    }

    if let Some(reqs) = &opts.requirements_path {
        let mut deps = JsonMap::new();
        deps.insert("requirements_path".into(), Value::String(reqs.clone()));
        root.insert("dependencies".into(), Value::Object(deps));
        notes.push(format!(
            "dependencies.requirements_path='{reqs}' (vendor it with uenv-hub/scripts/openenv-offline-precompile.sh)"
        ));
    }

    let mut iface_tbl = JsonMap::new();
    for (k, v) in [
        ("action", &interface.action),
        ("observation", &interface.observation),
        ("state", &interface.state),
    ] {
        if let Some(schema) = v {
            iface_tbl.insert(k.to_string(), schema.clone());
        }
    }
    if !iface_tbl.is_empty() {
        root.insert("interface".into(), Value::Object(iface_tbl));
    }

    let mut header = String::new();
    header.push_str(&format!(
        "# Generated by {CONVERTER_VERSION} from an OpenEnv project (openenv.yaml).\n"
    ));
    header.push_str(&format!(
        "# Source environment: {} (spec_version={}, runtime={})\n",
        spec.name,
        spec.spec_version.clone().unwrap_or_else(|| "-".into()),
        spec.runtime.clone().unwrap_or_else(|| "-".into()),
    ));
    header.push_str(
        "# 内网零外拉：[image].url 必须内网可达（内部 registry，或经 `uenv env publish-image`\n\
         # 托管到 Hub 后引用）；请勿改回 docker.io / ghcr.io 等公网仓库。\n\
         # 契约 [interface.*] 由 models.py 推导，修改实现后请重新导入以避免漂移。\n\n",
    );
    let mut body = String::new();
    emit_table("", &root, &mut body);

    Ok(Converted {
        env_type,
        version,
        interface,
        manifest_toml: format!("{header}{}", body.trim_start_matches('\n')),
        report,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real `openenv.yaml` from the `openenv/coding_env` Space: contract classes
    // are declared as bare scalars.
    const CODING_YAML: &str = r#"name: coding_env
version: "0.1.0"
description: "Coding environment for OpenEnv"
action: CodeAction
observation: CodeObservation
"#;

    // Real `openenv.yaml` from the `openenv/echo_env` Space: deployment-shaped,
    // no version / no contract classes.
    const ECHO_YAML: &str = r#"spec_version: 1
name: echo_env
type: space
runtime: fastapi
app: server.app:app
port: 8000
"#;

    // Real `models.py` from `openenv/coding_env`.
    const CODING_MODELS: &str = r#"
from openenv.core.env_server.interfaces import Action, Observation, State


class CodeAction(Action):
    """
    Represents a single code execution request.
    """

    code: str
    # Optional: future fields like 'lint': bool


class CodeObservation(Observation):
    """Result of executing code."""

    stdout: str = ""
    stderr: str = ""
    exit_code: int = 0


class CodeState(State):
    last_exit_code: int = 0
"#;

    // Real `models.py` from `openenv/echo_env` (pydantic `Field(...)`).
    const ECHO_MODELS: &str = r#"
from pydantic import Field
from openenv.core.env_server.types import Action, Observation


class EchoAction(Action):
    """Action for the Echo environment."""

    message: str = Field(..., min_length=1, description="Message to echo back")


class EchoObservation(Observation):
    echoed_message: str = Field(..., description="The echoed message from the environment")
    message_length: int = Field(default=0, ge=0, description="Length of the echoed message")
"#;

    #[test]
    fn parses_scalar_contract_refs() {
        let spec = OpenEnvSpec::from_yaml(CODING_YAML).unwrap();
        assert_eq!(spec.name, "coding_env");
        assert_eq!(spec.version.as_deref(), Some("0.1.0"));
        assert_eq!(spec.action.unwrap().class_name, "CodeAction");
        assert_eq!(spec.observation.unwrap().class_name, "CodeObservation");
    }

    #[test]
    fn parses_deployment_shaped_manifest() {
        let spec = OpenEnvSpec::from_yaml(ECHO_YAML).unwrap();
        assert_eq!(spec.name, "echo_env");
        assert_eq!(spec.spec_version.as_deref(), Some("1"));
        assert_eq!(spec.runtime.as_deref(), Some("fastapi"));
        assert_eq!(spec.app.as_deref(), Some("server.app:app"));
        assert_eq!(spec.port, Some(8000));
        assert!(spec.version.is_none(), "echo_env declares no version");
    }

    #[test]
    fn parses_nested_class_ref_form() {
        let yaml = r#"name: my_env
version: 0.1.0
action:
  class_name: MyAction
  module: my_env.models
observation:
  class_name: MyObservation
  module: my_env.models
default_image: my-env:latest
spec_version: 1
"#;
        let spec = OpenEnvSpec::from_yaml(yaml).unwrap();
        let action = spec.action.unwrap();
        assert_eq!(action.class_name, "MyAction");
        assert_eq!(action.module.as_deref(), Some("my_env.models"));
        assert_eq!(spec.default_image.as_deref(), Some("my-env:latest"));
    }

    #[test]
    fn rejects_unsupported_yaml_loudly() {
        let anchored = "name: x\nbase: &a 1\n";
        assert!(matches!(
            OpenEnvSpec::from_yaml(anchored),
            Err(OpenEnvError::UnsupportedYaml { .. })
        ));
        let tabbed = "name: x\nclient:\n\tmodule: a\n";
        assert!(matches!(
            OpenEnvSpec::from_yaml(tabbed),
            Err(OpenEnvError::UnsupportedYaml { .. })
        ));
        assert!(matches!(
            OpenEnvSpec::from_yaml("version: 1\n"),
            Err(OpenEnvError::MissingField("name"))
        ));
    }

    #[test]
    fn derives_contract_from_bare_annotations() {
        let classes = parse_python_models(CODING_MODELS);
        let action = classes.iter().find(|c| c.name == "CodeAction").unwrap();
        assert_eq!(action.fields.len(), 1);
        assert_eq!(action.fields[0].name, "code");
        assert!(action.fields[0].required, "bare annotation is required");

        let obs = classes.iter().find(|c| c.name == "CodeObservation").unwrap();
        let names: Vec<&str> = obs.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["stdout", "stderr", "exit_code"]);
        assert!(obs.fields.iter().all(|f| !f.required), "defaults → optional");
    }

    #[test]
    fn derives_contract_from_pydantic_field() {
        let classes = parse_python_models(ECHO_MODELS);
        let action = classes.iter().find(|c| c.name == "EchoAction").unwrap();
        let msg = &action.fields[0];
        assert_eq!(msg.name, "message");
        assert!(msg.required, "Field(...) is required");
        assert_eq!(msg.description.as_deref(), Some("Message to echo back"));

        let obs = classes.iter().find(|c| c.name == "EchoObservation").unwrap();
        let echoed = obs.fields.iter().find(|f| f.name == "echoed_message").unwrap();
        assert!(echoed.required);
        let len = obs.fields.iter().find(|f| f.name == "message_length").unwrap();
        assert!(!len.required, "Field(default=0) → optional");
    }

    #[test]
    fn injects_openenv_base_fields() {
        let classes = parse_python_models(CODING_MODELS);
        let obs = schema_for(&classes, None, Kind::Observation).unwrap();
        let props = obs.get("properties").unwrap();
        for f in ["done", "reward", "metadata", "stdout", "exit_code"] {
            assert!(props.get(f).is_some(), "observation must carry {f}");
        }
        let state = schema_for(&classes, None, Kind::State).unwrap();
        let sprops = state.get("properties").unwrap();
        for f in ["episode_id", "step_count", "last_exit_code"] {
            assert!(sprops.get(f).is_some(), "state must carry {f}");
        }
        // Action base contributes nothing, so only declared fields appear.
        let action = schema_for(&classes, None, Kind::Action).unwrap();
        assert_eq!(
            action.get("required").unwrap(),
            &serde_json::json!(["code"])
        );
    }

    #[test]
    fn python_types_map_to_json_schema() {
        assert_eq!(py_type_to_schema("str"), serde_json::json!({"type":"string"}));
        assert_eq!(py_type_to_schema("int"), serde_json::json!({"type":"integer"}));
        assert_eq!(py_type_to_schema("float"), serde_json::json!({"type":"number"}));
        assert_eq!(py_type_to_schema("bool"), serde_json::json!({"type":"boolean"}));
        assert_eq!(
            py_type_to_schema("list[str]"),
            serde_json::json!({"type":"array","items":{"type":"string"}})
        );
        assert_eq!(
            py_type_to_schema("Optional[str]"),
            serde_json::json!({"type":["string","null"]})
        );
        assert_eq!(py_type_to_schema("Dict[str, Any]"), serde_json::json!({"type":"object"}));
        // Unknown annotations stay permissive instead of guessing a wrong type.
        assert_eq!(py_type_to_schema("SomeCustom"), serde_json::json!({}));
    }

    #[test]
    fn conversion_rewrites_image_for_zero_egress() {
        let spec = OpenEnvSpec::from_yaml(
            "name: my_env\nversion: 0.2.0\ndefault_image: docker.io/openenv/my-env:latest\n",
        )
        .unwrap();
        let opts = ConvertOptions {
            internal_registry: Some("registry.uenv.internal/openenv".into()),
            ..Default::default()
        };
        let out = convert(&spec, None, &opts).unwrap();
        assert!(out.manifest_toml.contains("registry.uenv.internal/openenv/my_env:0.2.0"));
        // No *declared value* may reference the public registry. (The header
        // comment intentionally names docker.io as guidance; comments are not
        // part of the parsed manifest.)
        let declared: String = out
            .manifest_toml
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!declared.contains("docker.io"), "{declared}");
        assert!(out.notes.iter().any(|n| n.contains("零外拉")));
    }

    #[test]
    fn conversion_warns_on_public_registry_without_rewrite() {
        let spec = OpenEnvSpec::from_yaml(
            "name: my_env\nversion: 0.2.0\ndefault_image: ghcr.io/acme/my-env:1\n",
        )
        .unwrap();
        let out = convert(&spec, None, &ConvertOptions::default()).unwrap();
        assert!(
            out.report
                .issues
                .iter()
                .any(|i| i.location == "image.url" && i.message.contains("ghcr.io")),
            "{:?}",
            out.report.issues
        );
    }

    #[test]
    fn conversion_derives_entrypoint_and_health_path() {
        let spec = OpenEnvSpec::from_yaml(ECHO_YAML).unwrap();
        let out = convert(&spec, Some(ECHO_MODELS), &ConvertOptions::default()).unwrap();
        assert!(out
            .manifest_toml
            .contains("uvicorn server.app:app --host 0.0.0.0 --port 8000"));
        assert!(out.manifest_toml.contains("health_check_path = \"/health\""));
        // echo_env has no version → defaulted with a warning, never silently.
        assert_eq!(out.version, "0.1.0");
        assert!(out.report.issues.iter().any(|i| i.location == "version"));
    }

    /// The generated TOML must parse back into exactly the contract we derived.
    /// This is the correctness guarantee for the hand-written emitter.
    #[test]
    fn manifest_toml_round_trips() {
        let spec = OpenEnvSpec::from_yaml(CODING_YAML).unwrap();
        let out = convert(&spec, Some(CODING_MODELS), &ConvertOptions::default()).unwrap();

        let parsed: toml::Value = toml::from_str(&out.manifest_toml)
            .unwrap_or_else(|e| panic!("emitted TOML is invalid: {e}\n---\n{}", out.manifest_toml));
        let as_json: Value = serde_json::to_value(&parsed).unwrap();

        assert_eq!(as_json["env_type"], serde_json::json!("coding_env"));
        assert_eq!(
            as_json["interface"]["action"],
            *out.interface.action.as_ref().unwrap(),
            "action schema must survive the TOML round trip"
        );
        assert_eq!(
            as_json["interface"]["observation"],
            *out.interface.observation.as_ref().unwrap()
        );
        assert_eq!(
            as_json["interface"]["state"],
            *out.interface.state.as_ref().unwrap()
        );
    }

    #[test]
    fn emitted_manifest_passes_domain_validation() {
        use uenv_hub_types::{Dependencies, ImageSpec, PublishVersionRequest, ResourceSpec};
        let spec = OpenEnvSpec::from_yaml(CODING_YAML).unwrap();
        let opts = ConvertOptions {
            internal_registry: Some("registry.uenv.internal/openenv".into()),
            ..Default::default()
        };
        let out = convert(&spec, Some(CODING_MODELS), &opts).unwrap();
        let req = PublishVersionRequest {
            version: out.version.clone(),
            changelog: None,
            image: Some(ImageSpec {
                url: format!("registry.uenv.internal/openenv/{}:{}", out.env_type, out.version),
                digest: None,
                size_bytes: None,
                arch: Some("amd64".into()),
                base_image_ref: None,
            }),
            base_image: None,
            health_check_path: Some("/health".into()),
            entrypoint: Some("uvicorn server.app:app".into()),
            supported_backends: vec!["docker".into()],
            config_schema: None,
            default_config: None,
            resources: ResourceSpec::default(),
            interface: out.interface.clone(),
            examples: vec![],
            dependencies: Some(Dependencies::default()),
            min_uenv_version: None,
            rubric: None,
        };
        let report = crate::domain::manifest::validate_manifest(&out.env_type, &req);
        assert!(report.valid, "{:?}", report.issues);
        // Fully standardized: no zero-egress or missing-contract warnings.
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.location.starts_with("interface")),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn sanitizes_illegal_env_type_characters() {
        assert_eq!(sanitize_env_type("My Env!"), "my-env");
        assert_eq!(sanitize_env_type("echo_env"), "echo_env");
    }
}
