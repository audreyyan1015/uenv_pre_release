//! Benchmark runtime contract for SWE-like environments.
//!
//! This is the machine-readable layer between an EnvPackage/catalog row and the
//! Worker lifecycle. It describes how the initial workspace state is produced,
//! how gold patches are interpreted, and which reward adapter is authoritative.

use serde::{Deserialize, Serialize};

use crate::swe::variant::BenchmarkVariant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchMode {
    None,
    ApplyDatasetPatch,
    ReverseDatasetPatch,
}

impl Default for PatchMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchSemantics {
    AlreadyBuggy,
    BugToFix,
    CleanToBuggy,
}

impl Default for PatchSemantics {
    fn default() -> Self {
        Self::AlreadyBuggy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPatchBase {
    ImageState,
    ProvisionedState,
}

impl Default for ModelPatchBase {
    fn default() -> Self {
        Self::ProvisionedState
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPatchCollect {
    GitDiff,
}

impl Default for ModelPatchCollect {
    fn default() -> Self {
        Self::GitDiff
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardAdapterKind {
    InternalPytest,
    ExternalCommand,
    InternalWithExternalOverride,
}

impl Default for RewardAdapterKind {
    fn default() -> Self {
        Self::InternalPytest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct InitialStateContract {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub patch_semantics: PatchSemantics,
    #[serde(default)]
    pub provision_patch: PatchMode,
    #[serde(default)]
    pub commit_after_provision: bool,
}

impl Default for InitialStateContract {
    fn default() -> Self {
        Self {
            source: "image".to_string(),
            patch_semantics: PatchSemantics::AlreadyBuggy,
            provision_patch: PatchMode::None,
            commit_after_provision: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModelPatchContract {
    #[serde(default)]
    pub base: ModelPatchBase,
    #[serde(default)]
    pub collect: ModelPatchCollect,
}

impl Default for ModelPatchContract {
    fn default() -> Self {
        Self {
            base: ModelPatchBase::ProvisionedState,
            collect: ModelPatchCollect::GitDiff,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GoldContract {
    #[serde(default)]
    pub patch_mode: PatchMode,
}

impl Default for GoldContract {
    fn default() -> Self {
        Self {
            patch_mode: PatchMode::ApplyDatasetPatch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RewardContract {
    #[serde(default)]
    pub adapter: RewardAdapterKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_env: Option<String>,
    #[serde(default)]
    pub authority: String,
    #[serde(default)]
    pub fallback: String,
}

impl Default for RewardContract {
    fn default() -> Self {
        Self {
            adapter: RewardAdapterKind::InternalPytest,
            command_env: None,
            authority: "internal_pytest".to_string(),
            fallback: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BenchmarkRuntimeContract {
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<String>,
    #[serde(default)]
    pub initial_state: InitialStateContract,
    #[serde(default)]
    pub model_patch: ModelPatchContract,
    #[serde(default)]
    pub gold: GoldContract,
    #[serde(default)]
    pub reward: RewardContract,
}

impl BenchmarkRuntimeContract {
    pub fn for_variant(variant: BenchmarkVariant) -> Self {
        match variant {
            BenchmarkVariant::Pro => Self {
                kind: "swe".to_string(),
                workspace_dir: Some("/app".to_string()),
                initial_state: InitialStateContract {
                    source: "image".to_string(),
                    patch_semantics: PatchSemantics::BugToFix,
                    provision_patch: PatchMode::None,
                    commit_after_provision: false,
                },
                model_patch: ModelPatchContract::default(),
                gold: GoldContract {
                    patch_mode: PatchMode::ApplyDatasetPatch,
                },
                reward: RewardContract {
                    adapter: RewardAdapterKind::InternalWithExternalOverride,
                    command_env: Some("UENV_SWE_PRO_EVAL_CMD".to_string()),
                    authority: "swebench_pro".to_string(),
                    fallback: "internal_pytest_parser".to_string(),
                },
            },
            BenchmarkVariant::Smith => Self {
                kind: "swe".to_string(),
                workspace_dir: Some("/testbed".to_string()),
                initial_state: InitialStateContract {
                    source: "image".to_string(),
                    patch_semantics: PatchSemantics::CleanToBuggy,
                    provision_patch: PatchMode::ApplyDatasetPatch,
                    commit_after_provision: true,
                },
                model_patch: ModelPatchContract::default(),
                gold: GoldContract {
                    patch_mode: PatchMode::ReverseDatasetPatch,
                },
                reward: RewardContract {
                    adapter: RewardAdapterKind::InternalWithExternalOverride,
                    command_env: Some("UENV_SWE_SMITH_EVAL_CMD".to_string()),
                    authority: "official_swesmith".to_string(),
                    fallback: "internal_pytest_parser_diagnostic".to_string(),
                },
            },
            BenchmarkVariant::Verified | BenchmarkVariant::Lite => Self {
                kind: "swe".to_string(),
                workspace_dir: Some("/testbed".to_string()),
                initial_state: InitialStateContract {
                    source: "image".to_string(),
                    patch_semantics: PatchSemantics::BugToFix,
                    provision_patch: PatchMode::None,
                    commit_after_provision: false,
                },
                model_patch: ModelPatchContract::default(),
                gold: GoldContract {
                    patch_mode: PatchMode::ApplyDatasetPatch,
                },
                reward: RewardContract::default(),
            },
        }
    }

    pub fn merge_overlay(mut self, overlay: &BenchmarkRuntimeContract) -> Self {
        if !overlay.kind.trim().is_empty() {
            self.kind = overlay.kind.clone();
        }
        if overlay.workspace_dir.is_some() {
            self.workspace_dir = overlay.workspace_dir.clone();
        }
        self.initial_state = overlay.initial_state.clone();
        self.model_patch = overlay.model_patch.clone();
        self.gold = overlay.gold.clone();
        self.reward = overlay.reward.clone();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smith_contract_declares_bug_baseline_and_reverse_gold() {
        let c = BenchmarkRuntimeContract::for_variant(BenchmarkVariant::Smith);
        assert_eq!(
            c.initial_state.patch_semantics,
            PatchSemantics::CleanToBuggy
        );
        assert_eq!(
            c.initial_state.provision_patch,
            PatchMode::ApplyDatasetPatch
        );
        assert!(c.initial_state.commit_after_provision);
        assert_eq!(c.gold.patch_mode, PatchMode::ReverseDatasetPatch);
        assert_eq!(
            c.reward.command_env.as_deref(),
            Some("UENV_SWE_SMITH_EVAL_CMD")
        );
    }

    #[test]
    fn pro_contract_keeps_image_buggy_and_applies_gold_forward() {
        let c = BenchmarkRuntimeContract::for_variant(BenchmarkVariant::Pro);
        assert_eq!(c.workspace_dir.as_deref(), Some("/app"));
        assert_eq!(c.initial_state.provision_patch, PatchMode::None);
        assert_eq!(c.gold.patch_mode, PatchMode::ApplyDatasetPatch);
    }
}
