//! Inspect the launched native process's resolved configuration, without a model request.
use super::{CustomApiConfiguration, CustomApiSnapshot};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio::process::Command;

/// Native identity fields, shared by auth status and the existing initialize
/// response. Human-facing diagnostic sections are never authentication evidence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Identity {
    Official,
    Api(String),
    SignedOut,
    ThirdParty,
    #[default]
    Unknown,
}
impl Identity {
    pub fn from_auth_status(value: &Value) -> Self {
        if value["apiProvider"]
            .as_str()
            .is_some_and(|v| v != "firstParty")
        {
            return Self::ThirdParty;
        }
        match (value["loggedIn"].as_bool(), value["authMethod"].as_str()) {
            (Some(false), Some("none")) => Self::SignedOut,
            (Some(true), Some("claude.ai" | "oauth_token")) => Self::Official,
            (Some(true), Some("api_key" | "api_key_helper")) => value["apiKeySource"]
                .as_str()
                .map(|source| Self::Api(source.into()))
                .unwrap_or(Self::Unknown),
            (Some(true), Some("third_party")) => Self::ThirdParty,
            _ => Self::Unknown,
        }
    }
    pub fn from_initialize(value: &Value) -> Self {
        let account = &value["account"];
        if account["apiProvider"]
            .as_str()
            .is_some_and(|v| v != "firstParty")
        {
            return Self::ThirdParty;
        }
        match account["tokenSource"].as_str() {
            Some(
                "claude.ai"
                | "CLAUDE_CODE_OAUTH_TOKEN"
                | "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"
                | "CCR_OAUTH_TOKEN_FILE",
            ) => Self::Official,
            Some("ANTHROPIC_AUTH_TOKEN" | "apiKeyHelper") => {
                Self::Api(account["tokenSource"].as_str().unwrap().into())
            }
            // Native subscription auth omits tokenSource and supplies subscriptionType.
            // Its value can be null; email alone is never login evidence.
            None if account.get("subscriptionType").is_some() => Self::Official,
            Some("none") | None => match account["apiKeySource"].as_str() {
                Some(source) if source != "none" => Self::Api(source.into()),
                _ if account["tokenSource"] == "none" => Self::SignedOut,
                _ => Self::Unknown,
            },
            Some(source) => Self::Api(source.into()),
        }
    }
    pub fn official_login_status(&self) -> Option<&'static str> {
        match self {
            Self::Official => Some("signed_in"),
            Self::SignedOut => Some("signed_out"),
            _ => None,
        }
    }
}
pub fn validate_identity(snapshot: &CustomApiSnapshot, identity: &Identity) -> Result<()> {
    let expected = if matches!(
        snapshot.credential_source,
        super::native::CredentialSource::Helper { .. }
    ) {
        "apiKeyHelper"
    } else {
        snapshot.credential_source.claude_variable()
    };
    let matches = if snapshot.configuration.enabled() {
        matches!(identity, Identity::Api(source) if source == expected)
    } else {
        matches!(identity, Identity::Official)
    };
    // Login availability is owned by native authentication and model calls.
    // This boundary only rejects a positively identified selection conflict.
    ensure!(
        matches || matches!(identity, Identity::Unknown | Identity::SignedOut),
        "Claude Code 的原生认证来源与所选连接方式不一致，请检查原生认证或组织策略。"
    );
    Ok(())
}

pub fn configure(snapshot: &CustomApiSnapshot, command: &mut Command) -> Result<()> {
    configure_environment(snapshot, command)?;
    let settings = snapshot.claude_settings()?;
    let path = snapshot.write_artifact("claude-settings.json", &serde_json::to_vec(&settings)?)?;
    command.arg("--settings").arg(path);
    Ok(())
}

/// The execution owner merges the same key-free settings with its existing permission/Fast settings.
pub fn configure_environment(snapshot: &CustomApiSnapshot, command: &mut Command) -> Result<()> {
    snapshot.assert_current()?;
    let settings = snapshot.claude_settings()?;
    command.envs(
        settings["env"]
            .as_object()
            .context("Claude 环境配置无效。")?
            .iter()
            .map(|(key, value)| (key, value.as_str().unwrap_or_default())),
    );
    snapshot.configure_environment(command)?;
    if snapshot.configuration.enabled() {
        command.env("ANTHROPIC_CUSTOM_HEADERS", native_headers(snapshot)?);
    }
    Ok(())
}

pub fn validate(
    snapshot: &CustomApiSnapshot,
    settings: &Value,
    identity: &Identity,
    explicit: Option<&str>,
) -> Result<()> {
    let CustomApiConfiguration::ClaudeCode { models, .. } = &snapshot.configuration else {
        anyhow::bail!("Claude 自定义 API 类型不匹配。");
    };
    let requested = snapshot.claude_settings()?;
    let effective = settings
        .pointer("/effective/env")
        .and_then(Value::as_object)
        .context("当前 Claude Code 无法报告最终环境配置，不能确认自定义 API 已生效。")?;
    for (name, expected) in requested["env"]
        .as_object()
        .context("Claude 环境配置无效。")?
    {
        ensure!(
            effective.get(name) == Some(expected),
            "Claude Code 原生设置或组织策略覆盖了 {}；自定义 API 未生效。",
            name
        );
    }
    validate_identity(snapshot, identity)?;
    if snapshot.configuration.enabled() {
        let headers = native_headers(snapshot)?;
        let actual_headers = effective
            .get("ANTHROPIC_CUSTOM_HEADERS")
            .and_then(Value::as_str);
        ensure!(
            actual_headers.is_none()
                || actual_headers == Some(headers.as_str())
                || actual_headers == Some(snapshot.redactor()?.text(&headers).as_str()),
            "Claude Code 原生请求头被其他设置覆盖；请在原生来源解决冲突。"
        );
        let source = snapshot.credential_source.claude_variable();
        if let Some(key) = snapshot.key()? {
            let actual = effective.get(source).and_then(Value::as_str);
            // Private control may already have scrubbed the exact current key. Another
            // source using the same variable name must not pass this check.
            ensure!(
                actual == Some(key.as_str())
                    || actual == Some("[redacted]")
                    // Shell-only values are absent from get_settings; initialize
                    // independently reports the resolved native credential source.
                    || actual.is_none()
                        && matches!(
                            snapshot.credential_source,
                            super::native::CredentialSource::Environment { .. }
                        ),
                "Claude Code 原生设置覆盖了当前 Key；未继续使用其他凭据。"
            );
        }
    }
    let model = explicit.or_else(|| snapshot.configuration.default_model());
    let expected = match model {
        Some("haiku") if snapshot.configuration.enabled() => {
            (!models.haiku_model.is_empty()).then_some(models.haiku_model.as_str())
        }
        Some("sonnet") if snapshot.configuration.enabled() => {
            (!models.sonnet_model.is_empty()).then_some(models.sonnet_model.as_str())
        }
        Some("opus") if snapshot.configuration.enabled() => {
            (!models.opus_model.is_empty()).then_some(models.opus_model.as_str())
        }
        // Native compound aliases remain runtime-owned.
        Some("haiku" | "sonnet" | "opus" | "default" | "opusplan" | "sonnet[1m]" | "opus[1m]") => {
            None
        }
        id => id,
    };
    if let Some(expected) = expected {
        ensure!(
            settings.pointer("/applied/model").and_then(Value::as_str) == Some(expected),
            "Claude Code 的实际模型与本次选择不一致，请检查原生模型映射。"
        );
    }
    Ok(())
}

// Reuse existing native headers without copying their values to a derived settings file.
fn native_headers(snapshot: &CustomApiSnapshot) -> Result<String> {
    let settings = super::native::read_json(&snapshot.context.path())?;
    let headers = settings
        .pointer("/env/ANTHROPIC_CUSTOM_HEADERS")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| snapshot.context.env("ANTHROPIC_CUSTOM_HEADERS"))
        .unwrap_or_default();
    let key = snapshot.key()?;
    for line in headers.lines().filter(|line| !line.trim().is_empty()) {
        let (name, value) = line
            .split_once(':')
            .context("Claude Code 原生请求头格式无效，请在原生来源处理。")?;
        if matches!(
            name.trim().to_ascii_lowercase().as_str(),
            "authorization" | "x-api-key" | "api-key"
        ) {
            ensure!(
                key.as_ref().is_some_and(
                    |key| value.trim() == key || value.trim() == format!("Bearer {key}")
                ),
                "Claude Code 原生认证请求头与当前 Key 不一致；请在该来源解决冲突，未尝试备用凭据。"
            );
        }
    }
    Ok(headers)
}
