//! Native connection editing for Claude Code and Codex. No credential store is owned here.
use crate::{
    agent_profile::AdapterKind, command::canonical_json_digest, platform::private_storage,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub mod claude_native;
pub mod codex_catalog;
pub mod native;
pub mod native_edit;
mod native_resource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    OfficialLogin,
    CustomApi,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeApiModels {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning_model: String,
    #[serde(default)]
    pub haiku_model: String,
    #[serde(default)]
    pub sonnet_model: String,
    #[serde(default)]
    pub opus_model: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomApiModel {
    /// Editor identity only; never written into a native model catalog.
    #[serde(default)]
    pub row_id: String,
    pub id: String,
    #[serde(default)]
    pub display_name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum CustomApiConfiguration {
    #[serde(rename = "claude-code-cli")]
    ClaudeCode {
        mode: Option<ConnectionMode>,
        base_url: String,
        models: ClaudeApiModels,
    },
    #[serde(rename = "codex-cli")]
    Codex {
        mode: Option<ConnectionMode>,
        base_url: String,
        models: Vec<CustomApiModel>,
        default_model: String,
        default_row_id: Option<String>,
    },
}
impl CustomApiConfiguration {
    pub fn kind(&self) -> AdapterKind {
        match self {
            Self::ClaudeCode { .. } => AdapterKind::ClaudeCodeCli,
            Self::Codex { .. } => AdapterKind::CodexCli,
        }
    }
    pub fn mode(&self) -> Option<ConnectionMode> {
        match self {
            Self::ClaudeCode { mode, .. } | Self::Codex { mode, .. } => *mode,
        }
    }
    pub fn set_mode(&mut self, next: Option<ConnectionMode>) {
        match self {
            Self::ClaudeCode { mode, .. } | Self::Codex { mode, .. } => *mode = next,
        }
    }
    pub fn enabled(&self) -> bool {
        self.mode() == Some(ConnectionMode::CustomApi)
    }
    pub fn base_url(&self) -> &str {
        match self {
            Self::ClaudeCode { base_url, .. } | Self::Codex { base_url, .. } => base_url,
        }
    }
    pub fn default_model(&self) -> Option<&str> {
        if !self.enabled() {
            return None;
        }
        let value = match self {
            Self::ClaudeCode { models, .. } => &models.model,
            Self::Codex { default_model, .. } => default_model,
        };
        (!value.is_empty()).then_some(value.as_str())
    }
    pub fn validate(&mut self, kind: AdapterKind) -> Result<()> {
        self.validate_model_list(kind, true)
    }
    pub fn validate_model_list(
        &mut self,
        kind: AdapterKind,
        require_model_list: bool,
    ) -> Result<()> {
        ensure!(self.kind() == kind, "连接类型与当前智能体不一致。");
        let enabled = self.enabled();
        let base_url = match self {
            Self::ClaudeCode { base_url, .. } | Self::Codex { base_url, .. } => base_url,
        };
        *base_url = base_url.trim().to_owned();
        ensure!(base_url.len() <= 4096, "接口地址过长。");
        if enabled || !base_url.is_empty() {
            let url = url::Url::parse(base_url)
                .map_err(|_| anyhow::anyhow!("请输入有效的 HTTP 或 HTTPS 接口地址。"))?;
            ensure!(
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
                "接口地址必须使用 HTTP 或 HTTPS。"
            );
            ensure!(
                url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
                "接口地址不能包含账号、密码或 # 片段。"
            );
        }
        fn model(value: &mut String, required: bool) -> Result<()> {
            *value = value.trim().to_owned();
            ensure!(!required || !value.is_empty(), "请填写模型 ID。");
            ensure!(
                value.len() <= 512 && !value.chars().any(char::is_control),
                "模型 ID 包含无效字符或过长。"
            );
            Ok(())
        }
        match self {
            Self::ClaudeCode { models, .. } => {
                for value in [
                    &mut models.model,
                    &mut models.reasoning_model,
                    &mut models.haiku_model,
                    &mut models.sonnet_model,
                    &mut models.opus_model,
                ] {
                    model(value, false)?;
                }
            }
            Self::Codex {
                models,
                default_model,
                default_row_id,
                ..
            } => {
                ensure!(
                    models.len() <= 128 && (!enabled || !require_model_list || !models.is_empty()),
                    "请至少添加一个模型，最多 128 项。"
                );
                let mut ids = BTreeSet::new();
                let mut rows = BTreeSet::new();
                for row in models.iter_mut() {
                    model(&mut row.id, enabled)?;
                    ensure!(ids.insert(row.id.clone()), "模型 ID 不能重复。");
                    ensure!(
                        !row.row_id.is_empty()
                            && row.row_id.len() <= 128
                            && rows.insert(row.row_id.clone()),
                        "模型行标识无效。"
                    );
                    model(&mut row.display_name, false)?;
                }
                let selected = models
                    .iter()
                    .find(|row| Some(&row.row_id) == default_row_id.as_ref());
                ensure!(
                    !enabled || !require_model_list || selected.is_some(),
                    "请选择一个默认模型；删除默认项前请先指定新的默认项。"
                );
                *default_model = selected.map(|row| row.id.clone()).unwrap_or_default();
            }
        }
        Ok(())
    }
}
/// Write-only: never Debug/Serialize or part of command receipts.
#[derive(Default, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApiKeyChange {
    #[default]
    Keep,
    Replace {
        value: String,
    },
    Clear,
}
impl ApiKeyChange {
    pub fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
    pub fn validate(&self) -> Result<()> {
        if let Self::Replace { value } = self {
            ensure!(
                !value.is_empty()
                    && value.len() <= 8192
                    && value.bytes().all(|c| (0x21..=0x7e).contains(&c)),
                "API Key 不能为空，不能包含空白或控制字符。"
            );
            ensure!(
                !value.chars().all(|c| matches!(c, '*' | '•')),
                "请输入真实 API Key，不能保存掩码。"
            );
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCredential {
    pub status: String,
    pub source: String,
    pub source_label: String,
    pub version: String,
    pub source_writable: bool,
    pub can_replace: bool,
    pub can_clear: bool,
    pub restriction: Option<String>,
    pub remedy: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionObservation {
    pub initial_mode: Option<ConnectionMode>,
    pub login_status: String,
    pub conflict: Option<String>,
    pub login_command: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldEdit {
    pub path: Vec<String>,
    pub before: Value,
    pub after: Value,
    pub label: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConflict {
    #[serde(flatten)]
    pub edit: FieldEdit,
    pub current: Value,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomApiSnapshot {
    pub configuration: CustomApiConfiguration,
    /// Only a deliberately maintained native list restricts member selections.
    /// An inherited default model is never an allowlist.
    #[serde(default)]
    pub configured_model_ids: Option<Vec<String>>,
    pub context: native::NativeContext,
    pub native_revision: String,
    pub credential_version: String,
    pub credential_source: native::CredentialSource,
    pub provider_id: String,
    /// Compatibility for already-frozen records; never used to select a connection.
    #[serde(default, skip_serializing)]
    pub explicit_mode: bool,
}
impl std::fmt::Debug for CustomApiSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeConnectionSnapshot")
            .field("kind", &self.configuration.kind())
            .field("mode", &self.configuration.mode())
            .finish_non_exhaustive()
    }
}
impl CustomApiSnapshot {
    pub fn identity(&self) -> Result<String> {
        canonical_json_digest(&json!({
            "kind": self.configuration.kind(), "mode": self.configuration.mode(),
            "directory": self.context.directory, "connection": self.native_revision,
            "api": self.configuration.enabled().then_some(&self.configuration),
            "credential": self.configuration.enabled().then_some(&self.credential_version),
            "provider": self.configuration.enabled().then_some(&self.provider_id),
            "models": self.configured_model_ids,
        }))
    }
    pub fn key(&self) -> Result<Option<String>> {
        let (value, version) = native::credential_value(&self.credential_source, &self.context)?;
        ensure!(
            version == self.credential_version,
            "原生凭据已变化，此执行需要重新建立连接；未使用其他 Key。"
        );
        Ok(value)
    }
    pub fn redactor(&self) -> Result<CredentialRedactor> {
        let key = native::credential_value(&self.credential_source, &self.context)?.0;
        let mut secrets: Vec<_> = key.into_iter().collect();
        if self.context.kind == AdapterKind::ClaudeCodeCli {
            let settings = native::read_json(&self.context.path())?;
            if let Some(token) = settings
                .pointer("/env/CLAUDE_CODE_OAUTH_TOKEN")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| self.context.env("CLAUDE_CODE_OAUTH_TOKEN"))
            {
                secrets.push(token);
            }
        }
        Ok(CredentialRedactor(secrets))
    }
    pub fn assert_current(&self) -> Result<()> {
        let Ok(current) = native::read(&self.context, None) else {
            return Ok(());
        };
        ensure!(
            current.connection_revision == self.native_revision,
            "原生连接已变化，此执行需要重新建立连接；未恢复旧接口。"
        );
        Ok(())
    }
    pub fn artifact_path(&self, name: &str) -> Result<PathBuf> {
        ensure!(
            !name.contains(['/', '\\']) && !name.starts_with('.'),
            "无效的原生配置文件名。"
        );
        Ok(self
            .context
            .artifact_root
            .join(self.identity()?.trim_start_matches("sha256:"))
            .join(name))
    }
    pub fn write_artifact(&self, name: &str, contents: &[u8]) -> Result<PathBuf> {
        let path = self.artifact_path(name)?;
        if path.exists() {
            ensure!(
                std::fs::read(&path)? == contents,
                "此修订的原生文件已变化，拒绝覆盖。"
            );
        } else {
            private_storage::atomic_write_private_bytes(&path, contents)?;
        }
        Ok(path)
    }
    pub fn model_is_configured(&self, id: &str) -> bool {
        !self.configuration.enabled()
            || self
                .configured_model_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|value| value == id))
    }
}
/// Exact scrubbing at private native output boundaries; secrets never enter Debug or serialized state.
#[derive(Clone)]
pub struct CredentialRedactor(Vec<String>);
impl CredentialRedactor {
    pub fn text(&self, text: &str) -> String {
        self.0
            .iter()
            .filter(|key| !key.is_empty())
            .fold(text.to_owned(), |text, key| text.replace(key, "[redacted]"))
    }
    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => items.iter_mut().for_each(|item| self.value(item)),
            Value::Object(object) => {
                let old = std::mem::take(object);
                for (name, mut item) in old {
                    self.value(&mut item);
                    object.insert(self.text(&name), item);
                }
            }
            _ => {}
        }
    }
}

pub fn storage_root(database_path: &Path, kind: AdapterKind) -> Result<PathBuf> {
    Ok(database_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("本机数据目录不可用。"))?
        .join("runtime-api")
        .join(kind.as_str())
        .join("artifacts"))
}

#[cfg(test)]
mod tests;
