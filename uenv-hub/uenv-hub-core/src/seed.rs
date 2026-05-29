//! Seed data (L10): initial environments and official scaffold templates.
//!
//! Idempotent — safe to run on every startup. Existing environments are left
//! untouched; templates are upserted so scaffold updates propagate.

use crate::error::Result;
use crate::models::{NewEnv, NewManifest, NewTemplate};
use crate::repository::SqliteStore;
use crate::templates;
use serde_json::json;
use uenv_hub_types::{Dependencies, Example, ImageSpec, InterfaceSchema, ResourceSpec};

/// Seed the official scaffold templates into the DB.
pub async fn seed_templates(store: &SqliteStore) -> Result<()> {
    for tpl in templates::all() {
        let archive = templates::pack(&tpl)?;
        store
            .upsert_template(NewTemplate {
                name: tpl.name.to_string(),
                description: Some(tpl.description.to_string()),
                version: templates::TEMPLATE_VERSION.to_string(),
                archive,
            })
            .await?;
    }
    Ok(())
}

/// Seed example environments (math / code / agent) if they do not exist.
pub async fn seed_envs(store: &SqliteStore) -> Result<()> {
    if store.find_env_row("math").await?.is_none() {
        store
            .create_env(NewEnv {
                env_type: "math".into(),
                namespace: "default".into(),
                description: Some("Math problem-solving environment".into()),
                author: Some("uenv-team".into()),
                homepage: None,
                repository: None,
                license: Some("Apache-2.0".into()),
                tags: vec!["math".into(), "reasoning".into()],
            })
            .await?;
        store
            .publish_version("math", math_manifest())
            .await?;
    }

    if store.find_env_row("code").await?.is_none() {
        store
            .create_env(NewEnv {
                env_type: "code".into(),
                namespace: "default".into(),
                description: Some("Code-execution reward environment".into()),
                author: Some("uenv-team".into()),
                homepage: None,
                repository: None,
                license: Some("Apache-2.0".into()),
                tags: vec!["code".into(), "execution".into()],
            })
            .await?;
        store
            .publish_version("code", simple_manifest("code", "1.0.0"))
            .await?;
    }

    if store.find_env_row("agent").await?.is_none() {
        store
            .create_env(NewEnv {
                env_type: "agent".into(),
                namespace: "default".into(),
                description: Some("Multi-turn tool-using agent environment".into()),
                author: Some("uenv-team".into()),
                homepage: None,
                repository: None,
                license: Some("Apache-2.0".into()),
                tags: vec!["agent".into(), "multi-turn".into()],
            })
            .await?;
        store
            .publish_version("agent", simple_manifest("agent", "0.1.0"))
            .await?;
    }
    Ok(())
}

/// Seed everything (templates + envs).
pub async fn seed_all(store: &SqliteStore) -> Result<()> {
    seed_templates(store).await?;
    seed_envs(store).await?;
    Ok(())
}

fn math_manifest() -> NewManifest {
    NewManifest {
        version: "1.0.0".into(),
        changelog: Some("First release: algebra and geometry".into()),
        entrypoint: Some("uenv-worker math".into()),
        supported_backends: vec!["process".into(), "podman".into()],
        dependencies: Some(Dependencies {
            requirements_path: Some("requirements.txt".into()),
            install_script: None,
            requires: vec![],
        }),
        min_uenv_version: Some("0.1.0".into()),
        base_image: Some("uenv-base:latest".into()),
        health_check_path: Some("/health".into()),
        interface: InterfaceSchema {
            action: Some(json!({
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"]
            })),
            observation: Some(json!({
                "type": "object",
                "properties": {"question": {"type": "string"}, "done": {"type": "boolean"}}
            })),
            state: Some(json!({
                "type": "object",
                "properties": {"step": {"type": "integer"}, "score": {"type": "number"}}
            })),
        },
        examples: vec![Example {
            title: Some("easy single-step solve".into()),
            request: json!({"env_config": {"difficulty": "easy"}, "actions": [{"answer": "42"}]}),
        }],
        image: Some(ImageSpec {
            url: "registry.local/uenv/math:1.0.0".into(),
            digest: Some("sha256:0000000000000000000000000000000000000000000000000000000000000000".into()),
            size_bytes: Some(524288000),
            arch: Some("amd64".into()),
            base_image_ref: Some("uenv-base:latest".into()),
        }),
        config_schema: Some(json!({
            "type": "object",
            "properties": {"difficulty": {"type": "string", "enum": ["easy", "medium", "hard"]}}
        })),
        default_config: Some(json!({"difficulty": "easy"})),
        resources: ResourceSpec {
            cpu: Some(2.0),
            memory_mb: Some(4096),
            gpu: Some(0),
            gpu_type: None,
            disk_mb: None,
        },
        published_by: None,
    }
}

fn simple_manifest(env_type: &str, version: &str) -> NewManifest {
    NewManifest {
        version: version.into(),
        changelog: Some(format!("Initial {env_type} release")),
        entrypoint: Some(format!("uenv-worker {env_type}")),
        supported_backends: vec!["process".into()],
        dependencies: None,
        min_uenv_version: None,
        base_image: Some("uenv-base:latest".into()),
        health_check_path: Some("/health".into()),
        interface: InterfaceSchema {
            action: Some(json!({"type": "object"})),
            observation: Some(json!({"type": "object"})),
            state: Some(json!({"type": "object"})),
        },
        examples: vec![],
        image: Some(ImageSpec {
            url: format!("registry.local/uenv/{env_type}:{version}"),
            digest: None,
            size_bytes: None,
            arch: Some("amd64".into()),
            base_image_ref: Some("uenv-base:latest".into()),
        }),
        config_schema: Some(json!({"type": "object"})),
        default_config: Some(json!({})),
        resources: ResourceSpec {
            cpu: Some(1.0),
            memory_mb: Some(2048),
            gpu: Some(0),
            gpu_type: None,
            disk_mb: None,
        },
        published_by: None,
    }
}
