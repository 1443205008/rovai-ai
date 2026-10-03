//! Host-local custom connections. Public configuration contains no credential bytes.
//! Native files and process values are derived from one immutable connection snapshot.
use std::{collections::{BTreeMap, BTreeSet}, io::Read, path::{Path, PathBuf}};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{agent_profile::AdapterKind, command::canonical_json_digest, platform::private_storage};

pub mod codex_catalog;
pub mod claude_native;
pub mod grok_native;
mod native_resource;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeApiModels {
    #[serde(default)] pub model: String,
    #[serde(default)] pub reasoning_model: String,
    #[serde(default)] pub haiku_model: String,
    #[serde(default)] pub sonnet_model: String,
    #[serde(default)] pub opus_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomApiModel {
    pub id: String,
    #[serde(default)] pub display_name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KimiApiType { #[default] Kimi, Anthropic, Openai }

impl KimiApiType {
    pub fn as_str(self) -> &'static str {
        match self { Self::Kimi => "kimi", Self::Anthropic => "anthropic", Self::Openai => "openai" }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum CustomApiConfiguration {
    #[serde(rename = "claude-code-cli")]
    ClaudeCode { enabled: bool, base_url: String, models: ClaudeApiModels },
    #[serde(rename = "codex-cli")]
    Codex { enabled: bool, base_url: String, models: Vec<CustomApiModel>, default_model: String },
    #[serde(rename = "kimi-code-cli")]
    KimiCode { enabled: bool, base_url: String, api_type: KimiApiType, model: String },
    #[serde(rename = "grok-build")]
    GrokBuild { enabled: bool, base_url: String, model: String },
}

impl CustomApiConfiguration {
    pub fn kind(&self) -> AdapterKind {
        match self { Self::ClaudeCode { .. } => AdapterKind::ClaudeCodeCli, Self::Codex { .. } => AdapterKind::CodexCli,
            Self::KimiCode { .. } => AdapterKind::KimiCodeCli, Self::GrokBuild { .. } => AdapterKind::GrokBuild }
    }
    pub fn enabled(&self) -> bool {
        match self { Self::ClaudeCode { enabled, .. } | Self::Codex { enabled, .. } |
            Self::KimiCode { enabled, .. } | Self::GrokBuild { enabled, .. } => *enabled }
    }
    pub fn base_url(&self) -> &str {
        match self { Self::ClaudeCode { base_url, .. } | Self::Codex { base_url, .. } |
            Self::KimiCode { base_url, .. } | Self::GrokBuild { base_url, .. } => base_url }
    }
    pub fn default_model(&self) -> Option<&str> {
        match self {
            Self::ClaudeCode { models, .. } => (!models.model.is_empty()).then_some(models.model.as_str()),
            Self::Codex { default_model, .. } => Some(default_model),
            Self::KimiCode { model, .. } | Self::GrokBuild { model, .. } => Some(model),
        }
    }
    pub fn configured_model_ids(&self) -> Option<Vec<String>> {
        if !self.enabled() { return None; }
        match self {
            Self::Codex { models, .. } => Some(models.iter().map(|model| model.id.clone()).collect()),
            Self::KimiCode { model, .. } => Some(vec![model.clone()]),
            _ => None,
        }
    }
    pub fn validate(&mut self, kind: AdapterKind) -> Result<()> {
        ensure!(self.kind() == kind, "自定义 API 类型与当前智能体不一致。");
        let enabled = self.enabled();
        let base_url = match self { Self::ClaudeCode { base_url, .. } | Self::Codex { base_url, .. } |
            Self::KimiCode { base_url, .. } | Self::GrokBuild { base_url, .. } => base_url };
        *base_url = base_url.trim().to_owned();
        ensure!(base_url.len() <= 4096, "接口地址过长。");
        if enabled || !base_url.is_empty() {
            let url = url::Url::parse(base_url).map_err(|_| anyhow::anyhow!("请输入有效的 HTTP 或 HTTPS 接口地址。"))?;
            ensure!(matches!(url.scheme(), "http" | "https") && url.host_str().is_some(), "接口地址必须使用 HTTP 或 HTTPS。");
            ensure!(url.username().is_empty() && url.password().is_none(), "接口地址不能包含账号或密码。");
            ensure!(url.fragment().is_none(), "接口地址不能包含 # 片段。");
        }
        fn model(value: &mut String, required: bool) -> Result<()> {
            *value = value.trim().to_owned();
            ensure!(!required || !value.is_empty(), "请填写模型 ID。");
            ensure!(value.len() <= 512 && !value.chars().any(char::is_control), "模型 ID 包含无效字符或过长。");
            Ok(())
        }
        match self {
            Self::ClaudeCode { models, .. } => {
                for value in [&mut models.model, &mut models.reasoning_model, &mut models.haiku_model,
                    &mut models.sonnet_model, &mut models.opus_model] { model(value, false)?; }
            }
            Self::Codex { models, default_model, .. } => {
                ensure!(models.len() <= 128, "模型不能超过 128 项。");
                ensure!(!enabled || !models.is_empty(), "启用自定义 API 时至少需要一个模型。");
                let mut ids = BTreeSet::new();
                for item in models.iter_mut() {
                    model(&mut item.id, enabled)?;
                    ensure!(item.id.is_empty() || ids.insert(item.id.clone()), "模型 ID 不能重复。");
                    item.display_name = item.display_name.trim().to_owned();
                    ensure!(item.display_name.len() <= 512 && !item.display_name.chars().any(char::is_control), "模型显示名称无效或过长。");
                }
                model(default_model, enabled)?;
                ensure!(!enabled || ids.contains(default_model), "请选择列表中的一个默认模型；删除默认项前请先指定新的默认项。");
            }
            Self::KimiCode { model: value, .. } | Self::GrokBuild { model: value, .. } => model(value, enabled)?,
        }
        Ok(())
    }
    pub fn conflicts_with_environment(&self, name: &str) -> bool {
        if !self.enabled() { return false; }
        let name = name.to_ascii_uppercase();
        match self {
            Self::ClaudeCode { .. } => matches!(name.as_str(), "ANTHROPIC_BASE_URL" | "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY" |
                "ANTHROPIC_MODEL" | "ANTHROPIC_REASONING_MODEL" | "ANTHROPIC_DEFAULT_HAIKU_MODEL" |
                "ANTHROPIC_DEFAULT_SONNET_MODEL" | "ANTHROPIC_DEFAULT_OPUS_MODEL" | "ANTHROPIC_CUSTOM_HEADERS" |
                "CLAUDE_CODE_OAUTH_TOKEN" | "CLAUDE_CODE_USE_BEDROCK" | "CLAUDE_CODE_USE_VERTEX" | "CLAUDE_CODE_USE_FOUNDRY"),
            Self::Codex { .. } => matches!(name.as_str(), "OPENAI_API_KEY" | "OPENAI_BASE_URL" | "CODEX_API_KEY"),
            Self::KimiCode { .. } => name.starts_with("KIMI_MODEL_") || name == "KIMI_CODE_CUSTOM_HEADERS",
            Self::GrokBuild { .. } => matches!(name.as_str(), "XAI_API_KEY" | "GROK_CODE_XAI_API_KEY" | "GROK_XAI_API_BASE_URL" |
                "GROK_DEFAULT_MODEL" | "GROK_MODELS_BASE_URL" | "GROK_MODELS_LIST_URL"),
        }
    }
}

/// A write-only operation, deliberately neither Serialize nor Debug.
#[derive(Default, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApiKeyChange { #[default] Keep, Replace { value: String }, Clear }

impl ApiKeyChange {
    pub fn is_keep(&self) -> bool { matches!(self, Self::Keep) }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomApiState {
    pub revision: u64,
    pub credential_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomApiSnapshot {
    pub configuration: CustomApiConfiguration,
    pub revision: u64,
    pub credential_version: String,
    pub storage_root: PathBuf,
}

pub fn storage_root(database_path: &Path, kind: AdapterKind) -> Result<PathBuf> {
    Ok(database_path.parent().context("Host 数据目录不可用。")?.join("runtime-api").join(kind.as_str()))
}

fn credential_path(root: &Path, version: &str) -> Result<PathBuf> {
    ensure!(uuid::Uuid::parse_str(version).is_ok(), "自定义 API 凭据引用无效。");
    Ok(root.join("credentials").join(version).join("key"))
}

pub fn write_credential(root: &Path, value: &str) -> Result<String> {
    let value = value.trim();
    ensure!(!value.is_empty() && value.len() <= 8192 && value.bytes().all(|c| (0x21..=0x7e).contains(&c)),
        "API Key 不能为空，不能包含空白或控制字符。");
    ensure!(!value.chars().all(|c| matches!(c, '*' | '•')), "请输入真实 API Key，不能保存掩码。");
    private_storage::prepare_private_directory(root)?;
    let version = uuid::Uuid::new_v4().to_string();
    private_storage::atomic_write_private_bytes(&credential_path(root, &version)?, value.as_bytes())
        .map_err(|_| anyhow::anyhow!("无法写入 Host 私有 API Key 存储。"))?;
    Ok(version)
}

pub fn remove_credential(root: &Path, version: &str) -> Result<()> {
    let path = credential_path(root, version)?;
    match std::fs::remove_dir_all(path.parent().context("凭据目录无效。")?) {
        Ok(()) => Ok(()), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), Err(_) => anyhow::bail!("无法清理旧 API Key。"),
    }
}

/// The native temporary alias is private to the adapter; members select the saved API model ID.
pub fn project_kimi_session(snapshot: &CustomApiSnapshot, session: &mut Value) -> Result<()> {
    let id = snapshot.configuration.default_model().context("Kimi 默认模型缺失。")?;
    let current = session.pointer("/models/currentModelId").and_then(Value::as_str).or_else(||
        session.get("configOptions").and_then(Value::as_array)?.iter().find(|option| option["id"] == "model")?["currentValue"].as_str());
    ensure!(current == Some("__kimi_env_model__"), "Kimi Code 未使用本次自定义 API 临时模型，请检查原生版本或显式模型配置。");
    if let Some(models) = session.get_mut("models") {
        models["currentModelId"] = json!(id);
        if let Some(rows) = models["availableModels"].as_array_mut() {
            rows.retain(|row| row["modelId"] == "__kimi_env_model__");
            for row in rows { row["modelId"] = json!(id); row["name"] = json!(id); }
        }
    }
    if let Some(options) = session.get_mut("configOptions").and_then(Value::as_array_mut) {
        for option in options.iter_mut().filter(|option| option["id"] == "model") {
            option["currentValue"] = json!(id);
            if let Some(rows) = option["options"].as_array_mut() {
                rows.retain(|row| row["value"] == "__kimi_env_model__");
                for row in rows { row["value"] = json!(id); row["name"] = json!(id); }
            }
        }
    }
    Ok(())
}

impl CustomApiSnapshot {
    pub fn redactor(&self) -> Result<CredentialRedactor> { Ok(CredentialRedactor(self.key()?)) }
    pub fn identity(&self) -> Result<String> { canonical_json_digest(&serde_json::to_value(self)?) }
    pub fn key(&self) -> Result<String> {
        let mut value = String::new();
        private_storage::open_private_read_file(&credential_path(&self.storage_root, &self.credential_version)?)
            .and_then(|file| { file.take(8193).read_to_string(&mut value)?; Ok(()) })
            .map_err(|_| anyhow::anyhow!("无法读取此执行绑定的 API Key，请重新保存自定义 API 配置。"))?;
        ensure!(!value.is_empty() && value.len() <= 8192 && value.bytes().all(|c| (0x21..=0x7e).contains(&c)), "私有 API Key 格式无效。");
        Ok(value)
    }
    pub fn artifact_path(&self, name: &str) -> Result<PathBuf> {
        ensure!(!name.contains(['/', '\\']) && !name.starts_with('.'), "无效的原生配置文件名。");
        Ok(credential_path(&self.storage_root, &self.credential_version)?.parent().context("凭据目录无效。")?
            .join(self.identity()?.trim_start_matches("sha256:")).join(name))
    }
    pub fn write_artifact(&self, name: &str, contents: &[u8]) -> Result<PathBuf> {
        let path = self.artifact_path(name)?;
        if path.exists() {
            let mut prior = Vec::new();
            private_storage::open_private_read_file(&path)?.take(4 * 1024 * 1024).read_to_end(&mut prior)?;
            ensure!(prior == contents, "此修订的原生配置已变化，拒绝覆盖正在使用的文件。");
        } else { private_storage::atomic_write_private_bytes(&path, contents)?; }
        Ok(path)
    }
    pub fn model_is_configured(&self, id: &str) -> bool {
        match &self.configuration {
            CustomApiConfiguration::Codex { models, .. } => models.iter().any(|m| m.id == id),
            CustomApiConfiguration::KimiCode { model, .. } => id == model || id == "__kimi_env_model__",
            // Claude aliases and Grok native model registration remain native-owned.
            _ => true,
        }
    }
    pub fn environment(&self) -> Result<BTreeMap<String, String>> {
        let mut values = BTreeMap::new();
        let key = self.key()?;
        match &self.configuration {
            CustomApiConfiguration::ClaudeCode { base_url, models, .. } => {
                values.insert("ANTHROPIC_BASE_URL".into(), base_url.clone());
                values.insert("ANTHROPIC_AUTH_TOKEN".into(), key);
                for (name, value) in [("ANTHROPIC_MODEL", &models.model), ("ANTHROPIC_REASONING_MODEL", &models.reasoning_model),
                    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &models.haiku_model), ("ANTHROPIC_DEFAULT_SONNET_MODEL", &models.sonnet_model),
                    ("ANTHROPIC_DEFAULT_OPUS_MODEL", &models.opus_model)] {
                    if !value.is_empty() { values.insert(name.into(), value.clone()); }
                }
            }
            CustomApiConfiguration::Codex { .. } => { values.insert("ROVAI_CUSTOM_API_KEY".into(), key); }
            CustomApiConfiguration::KimiCode { base_url, api_type, model, .. } => {
                for (name, value) in [("KIMI_MODEL_NAME", model.clone()), ("KIMI_MODEL_PROVIDER_TYPE", api_type.as_str().into()),
                    ("KIMI_MODEL_BASE_URL", base_url.clone()), ("KIMI_MODEL_API_KEY", key)] { values.insert(name.into(), value); }
            }
            CustomApiConfiguration::GrokBuild { base_url, model, .. } => {
                for (name, value) in [("GROK_XAI_API_BASE_URL", base_url.clone()), ("GROK_DEFAULT_MODEL", model.clone()),
                    ("XAI_API_KEY", key)] { values.insert(name.into(), value); }
            }
        }
        Ok(values)
    }
    pub fn configure_environment(&self, command: &mut Command) -> Result<()> {
        command.envs(self.environment()?);
        Ok(())
    }
    pub fn claude_settings(&self) -> Result<Value> {
        ensure!(self.configuration.kind() == AdapterKind::ClaudeCodeCli, "自定义 API 类型不匹配。");
        let mut environment = self.environment()?;
        // Native --settings env overrides the same values in user/project settings.
        // Empty values disable these competing native authentication/provider paths.
        for key in ["ANTHROPIC_API_KEY", "ANTHROPIC_CUSTOM_HEADERS", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"] {
            environment.insert(key.into(), String::new());
        }
        Ok(json!({"env": environment}))
    }
}

/// Exact credential scrubbing at native output boundaries, including unlabelled errors.
/// Keep the value private and never include it in Debug, serialized state, or public diagnostics.
#[derive(Clone)]
pub struct CredentialRedactor(String);

impl CredentialRedactor {
    pub fn text(&self, text: &str) -> String { text.replace(&self.0, "[redacted]") }
    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => items.iter_mut().for_each(|item| self.value(item)),
            Value::Object(object) => {
                let old = std::mem::take(object);
                for (name, mut item) in old { self.value(&mut item); object.insert(self.text(&name), item); }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
