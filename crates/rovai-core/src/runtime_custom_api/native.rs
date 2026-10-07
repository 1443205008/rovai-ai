//! The native files remain authoritative. Projection contains references and digests, never keys.
use super::{
    ClaudeApiModels, ConnectionMode, ConnectionObservation, CustomApiConfiguration, CustomApiModel,
    CustomApiSnapshot, NativeCredential,
};
use crate::{
    agent_profile::AdapterKind, command::canonical_json_digest,
    runtime_startup::RuntimeStartupConfiguration,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeContext {
    pub kind: AdapterKind,
    pub directory: PathBuf,
    pub artifact_root: PathBuf,
    #[serde(default)]
    pub launcher: Option<String>,
    #[serde(default)]
    pub codex_source: Option<super::codex_source::Source>,
    #[serde(skip)]
    pub environment: BTreeMap<String, String>,
}
impl std::fmt::Debug for NativeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeContext")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
impl NativeContext {
    pub fn resolve(
        kind: AdapterKind,
        configuration: &RuntimeStartupConfiguration,
        database: &Path,
    ) -> Result<Self> {
        #[cfg(not(test))]
        let mut environment: BTreeMap<String, String> = std::env::vars().collect();
        // Library fixtures never inspect a developer's native credentials.
        #[cfg(test)]
        let mut environment: BTreeMap<String, String> = BTreeMap::from([(
            if cfg!(windows) { "USERPROFILE" } else { "HOME" }.into(),
            database
                .parent()
                .unwrap_or(Path::new("/tmp"))
                .join("isolated-native-home")
                .to_string_lossy()
                .into_owned(),
        )]);
        for entry in &configuration.environment {
            environment.insert(entry.name.clone(), entry.value.clone());
        }
        let name = if kind == AdapterKind::ClaudeCodeCli {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        let directory = if let Some(value) = environment.get(name).filter(|value| !value.is_empty())
        {
            PathBuf::from(value)
        } else {
            let home = environment
                .get(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .or_else(dirs::home_dir)
                .ok_or_else(|| anyhow::anyhow!("无法确定本机原生配置目录。"))?;
            home.join(if kind == AdapterKind::ClaudeCodeCli {
                ".claude"
            } else {
                ".codex"
            })
        };
        ensure!(
            directory.is_absolute(),
            "原生配置目录必须是绝对路径，请检查 {}。",
            name
        );
        let mut context = Self {
            kind,
            directory,
            artifact_root: super::storage_root(database, kind)?,
            launcher: configuration.program_path.clone().or_else(|| {
                #[cfg(not(test))]
                if kind == AdapterKind::CodexCli {
                    return crate::runtime_discovery::resolve_active_command_path("codex")
                        .map(|p| p.to_string_lossy().into_owned());
                }
                None
            }),
            codex_source: None,
            environment,
        };
        if kind == AdapterKind::CodexCli {
            context.codex_source = super::codex_source::resolve(&context);
        }
        Ok(context)
    }
    pub fn native_home(&self) -> &Path {
        self.codex_source
            .as_ref()
            .and_then(|s| s.base.parent())
            .unwrap_or(&self.directory)
    }
    pub fn path(&self) -> PathBuf {
        if let Some(source) = &self.codex_source {
            return source.profile.as_ref().unwrap_or(&source.base).clone();
        }
        self.directory
            .join(if self.kind == AdapterKind::ClaudeCodeCli {
                "settings.json"
            } else {
                "config.toml"
            })
    }
    pub fn env(&self, name: &str) -> Option<String> {
        if self.environment.is_empty() {
            crate::runtime_discovery::runtime_environment_variable(self.kind, name)
                .map(|v| v.to_string_lossy().into_owned())
        } else {
            self.environment.get(name).cloned()
        }
    }
    /// Local metadata commands must see the same launcher environment (notably
    /// the shell-discovered PATH used by npm shims) as the actual native process.
    pub fn for_command(&self, command: &tokio::process::Command) -> Self {
        let mut context = self.clone();
        if context.environment.is_empty() {
            context.environment = std::env::vars().collect();
        }
        for (name, value) in command.as_std().get_envs() {
            let name = name.to_string_lossy().into_owned();
            if cfg!(windows) {
                context
                    .environment
                    .retain(|key, _| !key.eq_ignore_ascii_case(&name));
            }
            if let Some(value) = value {
                context
                    .environment
                    .insert(name, value.to_string_lossy().into_owned());
            } else {
                context.environment.remove(&name);
            }
        }
        context
    }
    pub fn login_command(&self, program: Option<&str>) -> String {
        let claude = self.kind == AdapterKind::ClaudeCodeCli;
        let binary = if claude { "claude" } else { "codex" };
        let variable = if claude {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        let home = self
            .env(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .or_else(dirs::home_dir);
        let custom_directory = home.is_none_or(|home| {
            self.directory != home.join(if claude { ".claude" } else { ".codex" })
        });
        login_command(
            binary,
            program,
            custom_directory.then_some((variable, self.directory.to_string_lossy().as_ref())),
            cfg!(windows),
        )
    }
}

pub(super) fn login_command(
    binary: &str,
    program: Option<&str>,
    directory: Option<(&str, &str)>,
    windows: bool,
) -> String {
    let quote = |value: &str| {
        if windows {
            format!("'{}'", value.replace('\'', "''"))
        } else {
            format!("'{}'", value.replace('\'', "'\\''"))
        }
    };
    let command = program
        .map(|path| format!("{}{}", if windows { "& " } else { "" }, quote(path)))
        .unwrap_or_else(|| binary.into());
    let prefix = directory
        .map(|(name, value)| {
            if windows {
                format!("$env:{name}={}; ", quote(value))
            } else {
                format!("{name}={} ", quote(value))
            }
        })
        .unwrap_or_default();
    format!(
        "{prefix}{command}{}",
        if binary == "codex" { " login" } else { "" }
    )
}

// Only the existing native auth check populates this short-lived identity hint.
// It stores no credentials, starts no process and never participates in admission.
type LoginHints = std::collections::BTreeMap<PathBuf, (String, std::time::Instant, String)>;
static CLAUDE_LOGIN_HINTS: std::sync::OnceLock<std::sync::Mutex<LoginHints>> =
    std::sync::OnceLock::new();
fn login_evidence(directory: &Path) -> Result<String> {
    canonical_json_digest(&json!([
        read_bytes(&directory.join("settings.json"))?,
        read_bytes(&directory.join(".credentials.json"))?
    ]))
}
pub fn record_claude_login(payload: &Value, directory: Option<&Path>) {
    let Some(status) =
        super::claude_native::Identity::from_auth_status(payload).official_login_status()
    else {
        return; // An API check does not establish an official login identity.
    };
    let Some(directory) = directory.or_else(|| payload["configDirectory"].as_str().map(Path::new))
    else {
        return;
    };
    if !directory.is_absolute() {
        return;
    }
    if payload["configDirectory"]
        .as_str()
        .is_some_and(|path| Path::new(path) != directory)
    {
        return;
    }
    let Ok(evidence) = login_evidence(directory) else {
        return;
    };
    if let Ok(mut hints) = CLAUDE_LOGIN_HINTS.get_or_init(Default::default).lock() {
        hints.retain(|_, (_, time, _)| time.elapsed() < std::time::Duration::from_secs(300));
        hints.insert(
            directory.to_owned(),
            (evidence, std::time::Instant::now(), status.into()),
        );
    }
}
fn observed_claude_login(directory: &Path) -> Option<String> {
    let evidence = login_evidence(directory).ok()?;
    let hints = CLAUDE_LOGIN_HINTS.get()?.lock().ok()?;
    let (old, time, status) = hints.get(directory)?;
    (*old == evidence && time.elapsed() < std::time::Duration::from_secs(300))
        .then(|| status.clone())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialSource {
    Missing,
    Environment {
        name: String,
    },
    Json {
        path: PathBuf,
        pointer: String,
        variable: String,
    },
    Toml {
        path: PathBuf,
        keys: Vec<String>,
    },
    Helper {
        path: PathBuf,
    },
    CodexProvider {
        path: PathBuf,
        provider: String,
        mechanism: String,
    },
    ClaudeCloud {
        path: PathBuf,
        route: String,
    },
    NativeManaged {
        directory: PathBuf,
    },
}
impl CredentialSource {
    pub fn claude_variable(&self) -> &str {
        match self {
            Self::Environment { name } => name,
            Self::Json { variable, .. } => variable,
            _ => "ANTHROPIC_AUTH_TOKEN",
        }
    }
}
pub struct NativeRead {
    pub configuration: CustomApiConfiguration,
    pub credential: NativeCredential,
    pub observation: ConnectionObservation,
    pub revision: String,
    pub connection_revision: String,
    pub edit_revision: String,
    pub catalog_revision: Option<String>,
    pub source: CredentialSource,
    pub provider_id: String,
    pub catalog_path: Option<PathBuf>,
    pub configured_model_ids: Option<Vec<String>>,
}
impl NativeRead {
    pub fn snapshot(&self, context: &NativeContext, explicit_mode: bool) -> CustomApiSnapshot {
        CustomApiSnapshot {
            configuration: self.configuration.clone(),
            configured_model_ids: self.configured_model_ids.clone(),
            context: context.clone(),
            native_revision: self.connection_revision.clone(),
            credential_version: self.credential.version.clone(),
            credential_source: self.source.clone(),
            provider_id: self.provider_id.clone(),
            explicit_mode,
        }
    }
}
pub fn supported(kind: AdapterKind) -> bool {
    matches!(kind, AdapterKind::ClaudeCodeCli | AdapterKind::CodexCli)
}
pub fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => anyhow::bail!("无法读取原生配置，请检查文件权限：{}", path.display()),
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && metadata.len() <= 16 * 1024 * 1024,
                "原生配置文件无效或过大：{}",
                path.display()
            );
            std::fs::read(path)
                .map(Some)
                .map_err(|_| anyhow::anyhow!("无法读取原生配置：{}", path.display()))
        }
    }
}
pub fn read_json(path: &Path) -> Result<Value> {
    read_bytes(path)?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("原生 JSON 配置无法解析：{}", path.display()))
        })
        .unwrap_or_else(|| Ok(json!({})))
}
pub fn read_toml(path: &Path) -> Result<toml_edit::DocumentMut> {
    let bytes = read_bytes(path)?.unwrap_or_default();
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("原生 TOML 配置不是 UTF-8：{}", path.display()))?;
    text.parse()
        .map_err(|_| anyhow::anyhow!("原生 TOML 配置无法解析：{}", path.display()))
}
// The selected launch source owns precedence; a historical selector is not proof.
pub(super) fn codex_config(context: &NativeContext) -> Result<Value> {
    super::codex_source::read(context)
}
pub(super) fn codex_setting<'a>(doc: &'a Value, name: &str) -> &'a Value {
    &doc[name]
}
/// `auto` keeps keyring-first resolution inside Codex. This is only its file
/// fallback, never proof that the file is the selected credential source.
pub(super) fn codex_auth_file(context: &NativeContext, store: &str) -> Result<Value> {
    match store {
        "keyring" | "ephemeral" => Ok(json!({})),
        "auto" => {
            Ok(read_json(&context.native_home().join("auth.json")).unwrap_or_else(|_| json!({})))
        }
        _ => read_json(&context.native_home().join("auth.json")),
    }
}
fn toml_value(path: &Path, keys: &[String]) -> Result<Option<String>> {
    let doc = read_toml(path)?;
    let mut item = doc.as_item();
    for key in keys {
        let Some(next) = item.get(key) else {
            return Ok(None);
        };
        item = next;
    }
    Ok(item.as_str().filter(|v| !v.is_empty()).map(str::to_owned))
}
pub fn credential_value(
    source: &CredentialSource,
    context: &NativeContext,
) -> Result<(Option<String>, String)> {
    let (value, evidence) = match source {
        CredentialSource::Missing => (None, Value::Null),
        CredentialSource::Environment { name } => {
            (context.env(name).filter(|v| !v.is_empty()), json!(name))
        }
        CredentialSource::Json { path, pointer, .. } => (
            read_json(path)?
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_owned),
            json!([path, pointer]),
        ),
        CredentialSource::Toml { path, keys } => (toml_value(path, keys)?, json!([path, keys])),
        CredentialSource::Helper { path } => (
            None,
            read_json(path)?
                .get("apiKeyHelper")
                .cloned()
                .unwrap_or_default(),
        ),
        CredentialSource::CodexProvider {
            provider,
            mechanism,
            ..
        } => (
            None,
            codex_config(context)?["model_providers"][provider][mechanism].clone(),
        ),
        CredentialSource::ClaudeCloud { path, route } => (
            None,
            json!([path, route, claude_cloud_route(context, &read_json(path)?)]),
        ),
        // Keep selection opaque, but account for auto's real file fallback when
        // fencing API processes. Never turn a fallback key into an env override.
        CredentialSource::NativeManaged { directory } => {
            let config = codex_config(context)?;
            let store = config["cli_auth_credentials_store"]
                .as_str()
                .unwrap_or("file");
            let auth = codex_auth_file(context, store)?;
            let file_key = (auth["auth_mode"] != "chatgpt")
                .then(|| auth["OPENAI_API_KEY"].as_str().filter(|v| !v.is_empty()))
                .flatten();
            let account = file_key
                .is_none()
                .then(|| auth.pointer("/tokens/account_id"))
                .flatten();
            (
                None,
                json!({"directory":directory, "store":store, "fileKey":file_key,
                "account":account}),
            )
        }
    };
    let version =
        canonical_json_digest(&json!({"source": source, "value": value, "evidence": evidence}))?;
    Ok((value, version))
}
pub fn claude_models(models: &ClaudeApiModels) -> [(&'static str, &str); 5] {
    [
        ("ANTHROPIC_MODEL", &models.model),
        ("ANTHROPIC_REASONING_MODEL", &models.reasoning_model),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &models.haiku_model),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", &models.sonnet_model),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", &models.opus_model),
    ]
}
pub(crate) const CLAUDE_CLOUD_ROUTES: [(&str, &str, &str); 3] = [
    (
        "CLAUDE_CODE_USE_BEDROCK",
        "ANTHROPIC_BEDROCK_BASE_URL",
        "Amazon Bedrock",
    ),
    (
        "CLAUDE_CODE_USE_VERTEX",
        "ANTHROPIC_VERTEX_BASE_URL",
        "Google Vertex AI",
    ),
    (
        "CLAUDE_CODE_USE_FOUNDRY",
        "ANTHROPIC_FOUNDRY_BASE_URL",
        "Microsoft Foundry",
    ),
];
fn claude_cloud_route(context: &NativeContext, settings: &Value) -> Option<(String, String)> {
    let get = |name: &str| {
        settings["env"][name]
            .as_str()
            .map(str::to_owned)
            .or_else(|| context.env(name))
            .unwrap_or_default()
    };
    CLAUDE_CLOUD_ROUTES
        .iter()
        .find(|(flag, _, _)| matches!(get(flag).as_str(), "1" | "true"))
        .map(|(_, base, route)| (route.to_string(), get(base)))
}
fn credential(context: &NativeContext, source: &CredentialSource) -> Result<NativeCredential> {
    let (value, version) = credential_value(source, context)?;
    let available = value.is_some()
        || matches!(
            source,
            CredentialSource::Helper { .. }
                | CredentialSource::NativeManaged { .. }
                | CredentialSource::CodexProvider { .. }
                | CredentialSource::ClaudeCloud { .. }
        );
    let (kind, label, writable) = match source {
        CredentialSource::Missing => ("native_file", "未配置".into(), true),
        CredentialSource::Environment { name } => {
            ("environment_reference", format!("环境变量 {name}"), false)
        }
        CredentialSource::Json { path, .. } | CredentialSource::Toml { path, .. } => {
            ("native_file", path.display().to_string(), true)
        }
        CredentialSource::Helper { path } => (
            "native_managed",
            format!("{} 的 apiKeyHelper", path.display()),
            true,
        ),
        CredentialSource::NativeManaged { .. } => {
            ("native_managed", "Codex 原生凭据管理".into(), false)
        }
        CredentialSource::CodexProvider { mechanism, .. } => (
            "native_managed",
            if mechanism == "auth" {
                "由原生命令提供"
            } else {
                "由原生 AWS 认证提供"
            }
            .into(),
            true,
        ),
        CredentialSource::ClaudeCloud { route, .. } => ("native_cloud", route.clone(), false),
    };
    Ok(NativeCredential {
        status: if available {
            "available"
        } else if matches!(source, CredentialSource::Missing) {
            "missing"
        } else {
            "invalid_reference"
        }
        .into(),
        source: kind.into(),
        source_label: label,
        version,
        source_writable: writable,
        can_replace: true,
        can_clear: writable,
        restriction: None,
        remedy: (!writable)
            .then(|| "可输入新 Key 并保存到原生连接；清除原有引用请在该来源操作。".into()),
        value,
    })
}
pub fn read(context: &NativeContext, _selected: Option<ConnectionMode>) -> Result<NativeRead> {
    ensure!(supported(context.kind), "此智能体没有原生连接编辑入口。");
    let path = context.path();
    let (configuration, source, initial_mode, login_status, provider_id, catalog_path, evidence) =
        if context.kind == AdapterKind::ClaudeCodeCli {
            let settings = read_json(&path)?;
            ensure!(
                settings.is_object(),
                "Claude Code 原生设置必须是 JSON 对象。"
            );
            let get = |name: &str| {
                settings
                    .get("env")
                    .and_then(|e| e.get(name))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| context.env(name))
                    .unwrap_or_default()
            };
            let mut source = CredentialSource::Missing;
            for name in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
                if !get(name).is_empty() {
                    source = if settings.get("env").and_then(|e| e.get(name)).is_some() {
                        CredentialSource::Json {
                            path: path.clone(),
                            pointer: format!("/env/{name}"),
                            variable: name.into(),
                        }
                    } else {
                        CredentialSource::Environment { name: name.into() }
                    };
                    break;
                }
            }
            if matches!(source, CredentialSource::Missing)
                && settings["apiKeyHelper"]
                    .as_str()
                    .is_some_and(|v| !v.is_empty())
            {
                source = CredentialSource::Helper { path: path.clone() };
            }
            let cloud = claude_cloud_route(context, &settings);
            if let Some((route, _)) = &cloud {
                source = CredentialSource::ClaudeCloud {
                    path: path.clone(),
                    route: route.clone(),
                };
            }
            let base_url = cloud
                .as_ref()
                .map(|(_, url)| url.clone())
                .unwrap_or_else(|| get("ANTHROPIC_BASE_URL"));
            let native_api = !base_url.is_empty()
                || !matches!(source, CredentialSource::Missing)
                || cloud.is_some();
            let login = read_json(&context.directory.join(".credentials.json"))?;
            let observed_login = observed_claude_login(&context.directory);
            let login_status = if !get("CLAUDE_CODE_OAUTH_TOKEN").is_empty()
                || login["claudeAiOauth"]["accessToken"]
                    .as_str()
                    .is_some_and(|v| !v.is_empty())
            {
                "signed_in"
            } else {
                observed_login.as_deref().unwrap_or("unknown")
            };
            let models = ClaudeApiModels {
                model: {
                    let model = get("ANTHROPIC_MODEL");
                    if model.is_empty() {
                        settings["model"].as_str().unwrap_or_default().into()
                    } else {
                        model
                    }
                },
                reasoning_model: get("ANTHROPIC_REASONING_MODEL"),
                haiku_model: get("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
                sonnet_model: get("ANTHROPIC_DEFAULT_SONNET_MODEL"),
                opus_model: get("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            };
            // Connection selection and authenticated identity are separate observations.
            let initial = Some(if native_api {
                ConnectionMode::CustomApi
            } else {
                ConnectionMode::OfficialLogin
            });
            let config = CustomApiConfiguration::ClaudeCode {
                mode: initial,
                base_url: if native_api && cloud.is_none() && base_url.is_empty() {
                    "https://api.anthropic.com".into()
                } else {
                    base_url
                },
                models,
            };
            (
                config,
                source,
                initial,
                login_status.to_owned(),
                String::new(),
                None,
                settings,
            )
        } else {
            let doc: toml::Value = serde_json::from_value(codex_config(context)?)?;
            let get = |name: &str| doc.get(name);
            let provider_id = get("model_provider")
                .and_then(toml::Value::as_str)
                .unwrap_or("openai")
                .to_owned();
            // Built-in OpenAI is selected from the native registry, not an identically
            // named user table that Codex does not use.
            let provider = (provider_id != "openai")
                .then(|| doc.get("model_providers").and_then(|p| p.get(&provider_id)))
                .flatten();
            let ps = |name: &str| {
                provider
                    .and_then(|p| p.get(name))
                    .and_then(toml::Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            let mut base_url = ps("base_url");
            if provider_id == "openai" {
                base_url = get("openai_base_url")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| context.env("OPENAI_BASE_URL"))
                    .unwrap_or_default();
            }
            let auth_path = context.native_home().join("auth.json");
            let store = doc
                .get("cli_auth_credentials_store")
                .and_then(toml::Value::as_str)
                .unwrap_or("file");
            let managed_store = matches!(store, "keyring" | "auto" | "ephemeral");
            let auth = codex_auth_file(context, store)?;
            let native_login = provider_id == "openai"
                || provider
                    .and_then(|p| p.get("requires_openai_auth"))
                    .and_then(toml::Value::as_bool)
                    == Some(true);
            let mut source = CredentialSource::Missing;
            if provider
                .and_then(|p| p.get("auth"))
                .and_then(|a| a.get("command"))
                .and_then(toml::Value::as_str)
                .is_some_and(|v| !v.trim().is_empty())
            {
                source = CredentialSource::CodexProvider {
                    path: super::codex_source::field_file(
                        context,
                        &["model_providers", &provider_id],
                    ),
                    provider: provider_id.clone(),
                    mechanism: "auth".into(),
                };
            } else if provider.and_then(|p| p.get("aws")).is_some() {
                source = CredentialSource::CodexProvider {
                    path: super::codex_source::field_file(
                        context,
                        &["model_providers", &provider_id],
                    ),
                    provider: provider_id.clone(),
                    mechanism: "aws".into(),
                };
            } else if !ps("env_key").is_empty() {
                source = if ps("env_key") == "ROVAI_UNCONFIGURED_API_KEY" {
                    CredentialSource::Missing
                } else {
                    CredentialSource::Environment {
                        name: ps("env_key"),
                    }
                };
            } else if !ps("experimental_bearer_token").is_empty() {
                source = CredentialSource::Toml {
                    path: super::codex_source::field_file(
                        context,
                        &["model_providers", &provider_id, "experimental_bearer_token"],
                    ),
                    keys: vec![
                        "model_providers".into(),
                        provider_id.clone(),
                        "experimental_bearer_token".into(),
                    ],
                };
            } else if native_login && context.env("OPENAI_API_KEY").is_some_and(|v| !v.is_empty()) {
                source = CredentialSource::Environment {
                    name: "OPENAI_API_KEY".into(),
                };
            } else if native_login
                && !managed_store
                && auth["auth_mode"] != "chatgpt"
                && auth["OPENAI_API_KEY"]
                    .as_str()
                    .is_some_and(|v| !v.is_empty())
            {
                source = CredentialSource::Json {
                    path: auth_path,
                    pointer: "/OPENAI_API_KEY".into(),
                    variable: "OPENAI_API_KEY".into(),
                };
            } else if native_login
                && (managed_store
                    || auth["tokens"]["access_token"]
                        .as_str()
                        .is_some_and(|v| !v.is_empty()))
            {
                source = CredentialSource::NativeManaged {
                    directory: context.directory.clone(),
                };
            }
            let has_token = auth["tokens"]["access_token"]
                .as_str()
                .is_some_and(|v| !v.is_empty());
            let login_status = if managed_store {
                // A fallback file alone cannot prove which keyring/file identity Codex selects.
                "unknown"
            } else if has_token {
                "signed_in"
            } else if matches!(source, CredentialSource::NativeManaged { .. }) {
                "unknown"
            } else {
                "signed_out"
            };
            let native_api = provider_id != "openai"
                || !base_url.is_empty()
                || (!managed_store
                    && auth["auth_mode"] != "chatgpt"
                    && auth["OPENAI_API_KEY"]
                        .as_str()
                        .is_some_and(|v| !v.is_empty()))
                || !matches!(
                    source,
                    CredentialSource::Missing | CredentialSource::NativeManaged { .. }
                );
            // An official OAuth login alone is not a reusable API credential. Keep
            // its login status, but never offer it as the key for a newly entered URL.
            if !native_api && has_token && !managed_store {
                source = CredentialSource::Missing;
            }
            let default_model = get("model")
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let catalog_path = get("model_catalog_json")
                .and_then(toml::Value::as_str)
                .map(PathBuf::from)
                .map(|p| {
                    if p.is_absolute() {
                        p
                    } else {
                        super::codex_source::field_file(context, &["model_catalog_json"])
                            .parent()
                            .unwrap_or(context.native_home())
                            .join(p)
                    }
                });
            let mut models = Vec::new();
            let catalog = catalog_path.as_ref().map(|p| read_json(p)).transpose()?;
            if let Some(catalog) = &catalog {
                let entries = catalog["models"].as_array().ok_or_else(|| {
                    anyhow::anyhow!("Codex model_catalog_json 不是完整原生目录。")
                })?;
                for entry in entries.iter().filter(|e| {
                    catalog["rovai_model_ids"]
                        .as_array()
                        .map(|ids| ids.contains(&e["slug"]))
                        .unwrap_or_else(|| e["visibility"] == "list")
                }) {
                    if let Some(id) = entry["slug"].as_str() {
                        models.push(CustomApiModel {
                            row_id: row_id(id)?,
                            id: id.into(),
                            display_name: entry["display_name"].as_str().unwrap_or_default().into(),
                        });
                    }
                }
            }
            if !default_model.is_empty() && !models.iter().any(|m| m.id == default_model) {
                models.insert(
                    0,
                    CustomApiModel {
                        row_id: row_id(&default_model)?,
                        id: default_model.clone(),
                        display_name: String::new(),
                    },
                );
            }
            let default_row_id = models
                .iter()
                .find(|m| m.id == default_model)
                .map(|m| m.row_id.clone());
            let initial = if native_api
                || get("forced_login_method").and_then(toml::Value::as_str) == Some("api")
            {
                Some(ConnectionMode::CustomApi)
            } else if managed_store
                && doc.get("forced_login_method").and_then(toml::Value::as_str) != Some("chatgpt")
            {
                None // Native storage may contain either API or ChatGPT auth; do not guess.
            } else {
                Some(ConnectionMode::OfficialLogin)
            };
            let configuration = CustomApiConfiguration::Codex {
                mode: initial,
                base_url: if initial == Some(ConnectionMode::CustomApi) && base_url.is_empty() {
                    "https://api.openai.com/v1".into()
                } else {
                    base_url
                },
                models,
                default_model,
                default_row_id,
            };
            (
                configuration,
                source,
                initial,
                login_status.to_owned(),
                provider_id,
                catalog_path,
                json!({"config": doc, "catalog": catalog, "auth": auth}),
            )
        };
    // No read writes files or imports a key; stale Rovai mode metadata is ignored.
    if let Ok(url) = url::Url::parse(configuration.base_url()) {
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            "原生接口地址含账号或密码，无法安全回显；请在该原生来源改为独立凭据后重试。"
        );
    }
    let credential = credential(context, &source)?;
    let configured_model_ids = if configuration.mode() != Some(ConnectionMode::OfficialLogin)
        && evidence
            .pointer("/catalog/rovai_managed_model_list")
            .and_then(Value::as_bool)
            == Some(true)
    {
        match &configuration {
            CustomApiConfiguration::Codex { models, .. } => {
                Some(models.iter().map(|m| m.id.clone()).collect())
            }
            _ => None,
        }
    } else {
        None
    };
    let revision = canonical_json_digest(
        &json!({"configuration":configuration, "credential":credential.version, "native":evidence,
            "target":super::native_file::target(&path)?}),
    )?;
    let connection_revision = connection_digest(
        context,
        &configuration,
        &source,
        &credential.version,
        &provider_id,
        initial_mode,
        &evidence,
        true,
    )?;
    let edit_revision = connection_digest(
        context,
        &configuration,
        &source,
        &credential.version,
        &provider_id,
        initial_mode,
        &evidence,
        false,
    )?;
    let catalog_revision = catalog_path
        .as_ref()
        .map(|path| {
            canonical_json_digest(&json!([
                super::native_file::target(path)?,
                evidence["catalog"]
            ]))
        })
        .transpose()?;
    Ok(NativeRead {
        configuration,
        credential,
        observation: ConnectionObservation {
            initial_mode,
            login_status: if context
                .codex_source
                .as_ref()
                .is_some_and(|s| s.target_unconfirmed)
            {
                "unknown".into()
            } else {
                login_status.into()
            },
            conflict: super::codex_source::observation(context).or_else(|| {
                if context
                    .codex_source
                    .as_ref()
                    .is_some_and(|s| s.rejected_selector)
                    && evidence.pointer("/config/profile").is_some()
                {
                    Some(
                    "原生配置包含已停用的 profile 选择字段；保存连接时会移除，其他旧配置内容保留。"
                        .into(),
                )
                } else {
                    None
                }
            }),
            login_command: context.login_command(None),
        },
        revision,
        connection_revision,
        edit_revision,
        catalog_revision,
        source,
        provider_id,
        catalog_path,
        configured_model_ids,
    })
}
/// Execution compatibility is deliberately narrower than the whole-file edit CAS.
/// Only the selected connection participates; unrelated providers, UI, Skills,
/// MCP and native bookkeeping are owned by their existing runtime mechanisms.
fn connection_digest(
    context: &NativeContext,
    configuration: &CustomApiConfiguration,
    source: &CredentialSource,
    credential: &str,
    provider_id: &str,
    initial: Option<ConnectionMode>,
    evidence: &Value,
    execution: bool,
) -> Result<String> {
    let api = configuration.enabled();
    let native = if context.kind == AdapterKind::CodexCli {
        let doc = &evidence["config"];
        let store = doc["cli_auth_credentials_store"].as_str().unwrap_or("file");
        let mut provider = evidence["config"]["model_providers"][provider_id].clone();
        if let Some(fields) = provider.as_object_mut() {
            fields.remove("name");
            fields.remove("env_key_instructions");
            if matches!(source, CredentialSource::Environment { .. }) {
                fields.remove("experimental_bearer_token");
            }
        }
        let header_values = provider["env_http_headers"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(header, variable)| {
                variable
                    .as_str()
                    .map(|name| (header.clone(), context.env(name)))
            })
            .collect::<BTreeMap<_, _>>();
        let managed = !api || matches!(source, CredentialSource::NativeManaged { .. });
        json!({
            "provider": if api { provider } else { Value::Null },
            "headerEnvironment": if api { json!(header_values) } else { Value::Null },
            "catalog": if execution { execution_catalog(&evidence["catalog"]) } else { Value::Null },
            "officialModel": if execution && !api && initial != Some(ConnectionMode::CustomApi) { codex_setting(doc, "model").clone() } else { Value::Null },
            "restoresOfficialCatalog": !api && initial == Some(ConnectionMode::CustomApi),
            "authStore": if managed { json!(store) } else { Value::Null },
            // Token refresh and unrelated auth.json fields do not change account identity.
            // Auto/keyring selection remains native-owned, not inferred from a fallback file.
            "account": if managed && store == "file" { evidence.pointer("/auth/tokens/account_id").cloned().unwrap_or_default() } else { Value::Null },
            "legacySelector": doc["profile"],
            "forcedLogin": doc["forced_login_method"],
            "forcedWorkspace": doc["forced_chatgpt_workspace_id"],
        })
    } else {
        let env = |name: &str| {
            evidence["env"][name]
                .as_str()
                .map(str::to_owned)
                .or_else(|| context.env(name))
        };
        let models = match configuration {
            CustomApiConfiguration::ClaudeCode { models, .. } => models,
            _ => unreachable!(),
        };
        json!({
            "cloudRoute": claude_cloud_route(context, evidence),
            "headers": if api { json!(env("ANTHROPIC_CUSTOM_HEADERS")) } else { Value::Null },
            "officialToken": if api { Value::Null } else { json!(env("CLAUDE_CODE_OAUTH_TOKEN")) },
            "officialTokenDescriptor": if api { Value::Null } else { json!(env("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR")) },
            "officialModels": if execution && !api && initial != Some(ConnectionMode::CustomApi) { json!(models) } else { Value::Null },
            "resetsOfficialModel": execution && !api && initial == Some(ConnectionMode::CustomApi) && !models.model.is_empty(),
            "forcedLogin": evidence["forceLoginMethod"],
            "forcedOrg": evidence["forceLoginOrgUUID"],
        })
    };
    let api_fields = if execution {
        execution_configuration(configuration)
    } else {
        json!({"baseUrl":configuration.base_url()})
    };
    canonical_json_digest(&json!({"mode": configuration.mode(), "native": native,
        "target": super::native_file::target(&context.path())?,
        "baseTarget": if context.kind == AdapterKind::CodexCli { Some(super::native_file::target(&context.codex_source.as_ref().map(|s| s.base.clone()).unwrap_or_else(|| context.path()))?) } else { None },
        "api": if api { api_fields } else { Value::Null },
        "credential": if api || configuration.mode().is_none() && matches!(source, CredentialSource::NativeManaged { .. }) { json!(credential) } else { Value::Null }}))
}
pub fn row_id(id: &str) -> Result<String> {
    canonical_json_digest(&json!(id))
}

/// Labels and stable editor row IDs cannot change an execution's connection identity.
pub(super) fn execution_configuration(configuration: &CustomApiConfiguration) -> Value {
    let mut value = json!(configuration);
    if let CustomApiConfiguration::Codex { models, .. } = configuration {
        value["models"] = json!(models.iter().map(|m| &m.id).collect::<Vec<_>>());
        value.as_object_mut().unwrap().remove("defaultRowId");
    }
    value
}
fn execution_catalog(catalog: &Value) -> Value {
    let mut catalog = catalog.clone();
    if let Some(fields) = catalog.as_object_mut() {
        // Editor ownership is tracked separately by configured_model_ids; a
        // label save adding this marker cannot change the native connection.
        fields.remove("rovai_managed_model_list");
        fields.remove("rovai_model_ids");
    }
    if let Some(models) = catalog.get_mut("models").and_then(Value::as_array_mut) {
        for entry in models {
            if let Some(fields) = entry.as_object_mut() {
                fields.remove("display_name");
                fields.remove("description");
            }
        }
    }
    catalog
}
