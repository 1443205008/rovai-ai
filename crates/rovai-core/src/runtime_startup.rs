//! Machine-local launch preferences. Values never enter public Runtime evidence.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Result, bail, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{agent_profile::AdapterKind, db::Database, runtime_custom_api::{self, ApiKeyChange, CustomApiConfiguration, CustomApiSnapshot, CustomApiState}};

#[derive(Clone, Default, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeStartupConfiguration {
    pub program_path: Option<String>,
    #[serde(default)]
    pub environment: Vec<RuntimeEnvironmentVariable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_api: Option<CustomApiConfiguration>,
    /// Resolved by the Host from its private stored references, never accepted from clients.
    #[serde(skip)]
    pub custom_api_snapshot: Option<CustomApiSnapshot>,
}

impl PartialEq for RuntimeStartupConfiguration {
    fn eq(&self, other: &Self) -> bool {
        self.program_path == other.program_path && self.environment == other.environment && self.custom_api == other.custom_api
    }
}

impl std::fmt::Debug for RuntimeStartupConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeStartupConfiguration")
            .field("program_path", &self.program_path)
            .field("environment_count", &self.environment.len())
            .field("custom_api_enabled", &self.custom_api.as_ref().is_some_and(CustomApiConfiguration::enabled))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEnvironmentVariable {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStartupSettings {
    pub runtime_kind: AdapterKind,
    pub revision: u64,
    pub configuration: RuntimeStartupConfiguration,
    pub api_key_configured: bool,
}

impl RuntimeStartupConfiguration {
    pub fn validated(mut self, windows: bool) -> Result<Self> {
        if let Some(path) = &mut self.program_path {
            *path = path.trim().to_owned();
            ensure!(
                !path.is_empty() && path.len() <= 4096 && !path.contains(['\0', '\r', '\n']),
                "请选择有效的程序路径。"
            );
            ensure!(
                Path::new(path).is_absolute(),
                "程序路径必须是本机绝对路径。"
            );
        }
        ensure!(self.environment.len() <= 128, "环境变量不能超过 128 项。");
        let mut names = BTreeSet::new();
        let mut bytes = 0;
        for variable in &mut self.environment {
            variable.name = variable.name.trim().to_owned();
            let mut chars = variable.name.chars();
            ensure!(
                matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
                    && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && variable.name.len() <= 256,
                "变量名需以字母或下划线开头，只含字母、数字、下划线。"
            );
            ensure!(
                !variable.name.to_ascii_uppercase().starts_with("ROVAI_"),
                "ROVAI_ 开头的变量由应用管理，请使用其他变量名。"
            );
            let name = if windows {
                variable.name.to_ascii_uppercase()
            } else {
                variable.name.clone()
            };
            ensure!(names.insert(name), "变量名重复，请合并为一项。");
            ensure!(
                !variable.value.contains('\0') && variable.value.len() <= 65536,
                "变量值包含空字符或超过长度限制。"
            );
            bytes += variable.name.len() + variable.value.len();
        }
        ensure!(bytes <= 128 * 1024, "环境变量总长度超过限制。");
        if let Some(api) = &mut self.custom_api {
            api.validate(api.kind())?;
            for entry in &self.environment {
                ensure!(!api.conflicts_with_environment(&entry.name), "环境变量 {} 与自定义 API 重复，请先明确保留一种配置来源。", entry.name);
            }
        }
        Ok(self)
    }
}

fn load_record(database: &Database, runtime_kind: AdapterKind) -> Result<(RuntimeStartupSettings, Option<CustomApiState>)> {
    let row: Option<(i64, String)> = database.connection().query_row(
        "SELECT revision, configuration_json FROM runtime_startup_setting WHERE runtime_kind = ?1",
        [runtime_kind.as_str()], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let (revision, mut configuration, state) = match row {
        Some((revision, json)) => {
            let mut value: serde_json::Value = serde_json::from_str(&json)?;
            let state = value.as_object_mut().and_then(|value| value.remove("_customApiState"))
                .map(serde_json::from_value::<CustomApiState>).transpose()?;
            (u64::try_from(revision)?, serde_json::from_value::<RuntimeStartupConfiguration>(value)?, state)
        }
        None => (0, RuntimeStartupConfiguration::default(), None),
    };
    if let Some(api) = configuration.custom_api.as_ref().filter(|api| api.enabled()) {
        let state = state.as_ref().ok_or_else(|| anyhow::anyhow!("自定义 API 缺少私有凭据引用。"))?;
        configuration.custom_api_snapshot = Some(CustomApiSnapshot {
            configuration: api.clone(), revision: state.revision,
            credential_version: state.credential_version.clone().ok_or_else(|| anyhow::anyhow!("启用自定义 API 需要 API Key。"))?,
            storage_root: runtime_custom_api::storage_root(database.path(), runtime_kind)?,
        });
    }
    let api_key_configured = state.as_ref().is_some_and(|state| state.credential_version.is_some());
    Ok((RuntimeStartupSettings {
        runtime_kind,
        revision,
        configuration,
        api_key_configured,
    }, state))
}

pub fn load(database: &Database, runtime_kind: AdapterKind) -> Result<RuntimeStartupSettings> {
    Ok(load_record(database, runtime_kind)?.0)
}

pub fn load_all(database: &Database) -> Result<BTreeMap<AdapterKind, RuntimeStartupConfiguration>> {
    let mut configurations = BTreeMap::new();
    for kind in AdapterKind::ALL {
        let settings = load(database, kind)?;
        if settings.revision != 0 {
            configurations.insert(kind, settings.configuration.validated(cfg!(windows))?);
        }
    }
    Ok(configurations)
}

/// The row and old readiness evidence change together; an active process keeps
/// its already-captured environment. CAS prevents a stale editor overwriting it.
pub fn save(
    database: &mut Database,
    kind: AdapterKind,
    expected_revision: u64,
    configuration: RuntimeStartupConfiguration,
    search_generation: u64,
) -> Result<RuntimeStartupSettings> {
    save_with_api_key(database, kind, expected_revision, configuration, search_generation, ApiKeyChange::Keep)
}

pub fn save_with_api_key(
    database: &mut Database,
    kind: AdapterKind,
    expected_revision: u64,
    configuration: RuntimeStartupConfiguration,
    search_generation: u64,
    key_change: ApiKeyChange,
) -> Result<RuntimeStartupSettings> {
    let configuration = configuration.validated(cfg!(windows))?;
    ensure!(key_change.is_keep() || matches!(kind, AdapterKind::ClaudeCodeCli | AdapterKind::CodexCli | AdapterKind::KimiCodeCli | AdapterKind::GrokBuild), "此智能体没有自定义 API 密钥入口。");
    ensure!(!matches!(key_change, ApiKeyChange::Replace { .. }) || configuration.custom_api.is_some(), "替换 API Key 时需要同时提交自定义 API 配置。");
    if let Some(api) = &configuration.custom_api { ensure!(api.kind() == kind, "自定义 API 类型与当前智能体不一致。"); }
    let (current, current_state) = load_record(database, kind)?;
    if current.revision > 0 && current.configuration == configuration && key_change.is_keep() {
        return Ok(current);
    }
    if current.revision != expected_revision {
        bail!("启动设置已被更新，请重新读取后再保存。");
    }
    let revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("启动设置版本超出范围。"))?;
    let enabled = configuration.custom_api.as_ref().is_some_and(CustomApiConfiguration::enabled);
    let root = runtime_custom_api::storage_root(database.path(), kind)?;
    let prior_credential = current_state.as_ref().and_then(|state| state.credential_version.clone());
    ensure!(!enabled || !matches!(key_change, ApiKeyChange::Clear), "请先关闭自定义 API，再明确清除 API Key。");
    ensure!(!enabled || prior_credential.is_some() || matches!(key_change, ApiKeyChange::Replace { .. }), "启用自定义 API 需要已保存或新输入的 API Key。");
    let new_credential = match key_change {
        ApiKeyChange::Keep => None,
        ApiKeyChange::Clear => Some(None),
        ApiKeyChange::Replace { value } => Some(Some(runtime_custom_api::write_credential(&root, &value)?)),
    };
    let credential_version = new_credential.clone().unwrap_or_else(|| prior_credential.clone());
    let mut stored = serde_json::to_value(&configuration)?;
    if configuration.custom_api.is_some() || current_state.is_some() || credential_version.is_some() {
        stored["_customApiState"] = serde_json::to_value(CustomApiState { revision, credential_version })?;
    }
    let result = (|| -> Result<()> {
    let transaction = database.connection_mut().transaction()?;
    transaction.execute(
        "INSERT INTO runtime_startup_setting(runtime_kind, revision, configuration_json, updated_at)
         VALUES (?1, ?2, ?3, datetime('now')) ON CONFLICT(runtime_kind) DO UPDATE SET
         revision = excluded.revision, configuration_json = excluded.configuration_json, updated_at = excluded.updated_at",
        params![kind.as_str(), i64::try_from(revision)?, serde_json::to_string(&stored)?],
    )?;
    transaction.execute("UPDATE adapter_capability_snapshot SET stale_at = COALESCE(stale_at, datetime('now')),
        authentication_status='unknown', probe_status='installed_unverified', last_error='runtime_startup_configuration_changed'
        WHERE installation_id IN (SELECT id FROM adapter_installation WHERE adapter_kind = ?1 AND installation_class = 'managed_default')", [kind.as_str()])?;
    transaction.execute("UPDATE adapter_installation SET generation = generation + 1, version = version + 1,
        updated_at = datetime('now') WHERE adapter_kind = ?1 AND installation_class = 'managed_default'", [kind.as_str()])?;
    transaction.execute("INSERT INTO runtime_search_environment_state(singleton, generation, captured_at) VALUES(1, ?1, datetime('now')) ON CONFLICT(singleton) DO UPDATE SET generation=excluded.generation, captured_at=excluded.captured_at", [i64::try_from(search_generation)?])?;
    transaction.commit()?;
    Ok(())
    })();
    if let Err(error) = result {
        if let Some(Some(version)) = new_credential { let _ = runtime_custom_api::remove_credential(&root, &version); }
        return Err(error);
    }
    // A committed save must not appear to fail because deferred cleanup failed.
    let _ = cleanup_unused_credentials(database);
    load(database, kind)
}

pub(crate) fn snapshot_from_connection(connection: &rusqlite::Connection, kind: AdapterKind) -> Result<Option<CustomApiSnapshot>> {
    let json: Option<String> = connection.query_row(
        "SELECT configuration_json FROM runtime_startup_setting WHERE runtime_kind = ?1",
        [kind.as_str()], |row| row.get(0)).optional()?;
    let Some(json) = json else { return Ok(None); };
    let mut value: serde_json::Value = serde_json::from_str(&json)?;
    let state = value.as_object_mut().and_then(|value| value.remove("_customApiState"));
    let configuration: RuntimeStartupConfiguration = serde_json::from_value(value)?;
    let Some(configuration) = configuration.custom_api.filter(CustomApiConfiguration::enabled) else { return Ok(None); };
    let state: CustomApiState = serde_json::from_value(state.ok_or_else(|| anyhow::anyhow!("自定义 API 缺少私有凭据引用。"))?)?;
    Ok(Some(CustomApiSnapshot {
        configuration, revision: state.revision,
        credential_version: state.credential_version.ok_or_else(|| anyhow::anyhow!("自定义 API 缺少 API Key。"))?,
        storage_root: runtime_custom_api::storage_root(Path::new(connection.path().ok_or_else(|| anyhow::anyhow!("Host 数据目录不可用。"))?), kind)?,
    }))
}

/// The preview's owner must retain this guard until the native check has stopped.
pub struct DraftCredential { root: std::path::PathBuf, version: String }
impl Drop for DraftCredential {
    fn drop(&mut self) { let _ = runtime_custom_api::remove_credential(&self.root, &self.version); }
}

pub fn resolve_draft(database: &Database, kind: AdapterKind, mut configuration: RuntimeStartupConfiguration, change: ApiKeyChange) -> Result<(RuntimeStartupConfiguration, Option<DraftCredential>)> {
    configuration = configuration.validated(cfg!(windows))?;
    let Some(api) = configuration.custom_api.as_ref() else { return Ok((configuration, None)); };
    ensure!(api.kind() == kind, "自定义 API 类型与当前智能体不一致。");
    if !api.enabled() { return Ok((configuration, None)); }
    let (saved, state) = load_record(database, kind)?;
    let root = runtime_custom_api::storage_root(database.path(), kind)?;
    let (root, version, guard) = match change {
        ApiKeyChange::Keep => {
            let version = state.and_then(|s| s.credential_version).ok_or_else(|| anyhow::anyhow!("启用自定义 API 需要 API Key。"))?;
            let saved = CustomApiSnapshot { configuration: api.clone(), revision: saved.revision, credential_version: version, storage_root: root.clone() };
            let root = root.join("drafts");
            let version = runtime_custom_api::write_credential(&root, &saved.key()?)?;
            let guard = DraftCredential { root: root.clone(), version: version.clone() };
            (root, version, Some(guard))
        },
        ApiKeyChange::Clear => anyhow::bail!("请先关闭自定义 API，再明确清除 API Key。"),
        ApiKeyChange::Replace { value } => {
            let root = root.join("drafts");
            let version = runtime_custom_api::write_credential(&root, &value)?;
            let guard = DraftCredential { root: root.clone(), version: version.clone() };
            (root, version, Some(guard))
        }
    };
    configuration.custom_api_snapshot = Some(CustomApiSnapshot {
        configuration: api.clone(), revision: saved.revision.saturating_add(1), credential_version: version, storage_root: root,
    });
    Ok((configuration, guard))
}

/// Old keys are retained only while a saved setting or a non-terminal frozen Run needs them.
/// Historical Run references do not retain credentials or permit replay.
pub fn cleanup_unused_credentials(database: &Database) -> Result<()> {
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli, AdapterKind::KimiCodeCli, AdapterKind::GrokBuild] {
        let (_, state) = load_record(database, kind)?;
        let current = state.and_then(|state| state.credential_version);
        let root = runtime_custom_api::storage_root(database.path(), kind)?;
        let entries = match std::fs::read_dir(root.join("credentials")) {
            Ok(entries) => entries, Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let version = entry.file_name().to_string_lossy().into_owned();
            if uuid::Uuid::parse_str(&version).is_err() || current.as_deref() == Some(version.as_str()) { continue; }
            let retained: bool = database.connection().query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_run WHERE status NOT IN ('succeeded', 'failed', 'cancelled') AND json_extract(effective_config_json, '$.runtime.customApi.credentialVersion') = ?1)",
                [&version], |row| row.get(0))?;
            if !retained { runtime_custom_api::remove_credential(&root, &version)?; }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // New editor-input boundary; no database/process fixture is needed for this matrix.
    #[test]
    fn environment_validation_preserves_values_and_rejects_ambiguous_or_reserved_names() {
        let config = |names: &[&str]| RuntimeStartupConfiguration {
            custom_api: None,
            custom_api_snapshot: None,
            program_path: None,
            environment: names
                .iter()
                .map(|name| RuntimeEnvironmentVariable {
                    name: (*name).into(),
                    value: "  private-value  ".into(),
                })
                .collect(),
        };
        let valid = config(&[" HTTP_PROXY ", "_EMPTY"])
            .validated(false)
            .unwrap();
        assert_eq!(valid.environment[0].name, "HTTP_PROXY");
        assert_eq!(valid.environment[0].value, "  private-value  ");
        assert!(!format!("{valid:?}").contains("private-value"));
        for names in [
            &[""][..],
            &["1KEY"],
            &["BAD-NAME"],
            &["ROVAI_CONTEXT"],
            &["rovai_context"],
            &["KEY", " KEY "],
        ] {
            assert!(config(names).validated(false).is_err(), "{names:?}");
        }
        assert!(config(&["KEY", "key"]).validated(false).is_ok());
        assert!(config(&["KEY", "key"]).validated(true).is_err());
        let mut invalid = config(&["KEY"]);
        invalid.environment[0].value = "value\0tail".into();
        assert!(invalid.validated(false).is_err());
        assert!(
            serde_json::from_value::<RuntimeStartupConfiguration>(
                serde_json::json!({"environment": [], "system": true})
            )
            .is_err()
        );
    }

    // Runtime-local scope and cancellation restoration are new launch semantics. Two
    // concurrent scopes prove isolation without mutating the test runner's environment.
    #[tokio::test]
    async fn runtime_overlays_are_scoped_and_do_not_modify_the_parent_or_other_runtimes() {
        use crate::runtime_discovery::{
            RuntimeSearchEnvironment, configure_runtime_command, runtime_environment_variable,
            with_runtime_configuration,
        };
        let key = "STARTUP_ISOLATION_TEST_SENTINEL";
        let inherited = std::env::var_os(key);
        let scope = |value: &str| {
            RuntimeSearchEnvironment::for_test_paths(1, Vec::new()).with_startup_configuration(
                AdapterKind::CodexCli,
                RuntimeStartupConfiguration {
                    custom_api: None,
                    custom_api_snapshot: None,
                    program_path: None,
                    environment: vec![RuntimeEnvironmentVariable {
                        name: key.into(),
                        value: value.into(),
                    }],
                },
            )
        };
        let first = scope("first-private-value");
        let second = scope("second-private-value");
        let check = |search: RuntimeSearchEnvironment, expected: &'static str| async move {
            with_runtime_configuration(AdapterKind::CodexCli, &search, async {
                tokio::task::yield_now().await;
                let mut command = tokio::process::Command::new("fixture-only-never-spawned");
                configure_runtime_command(AdapterKind::CodexCli, &mut command);
                assert_eq!(
                    command
                        .as_std()
                        .get_envs()
                        .find(|(name, _)| *name == key)
                        .unwrap()
                        .1,
                    Some(std::ffi::OsStr::new(expected))
                );
                assert_eq!(
                    runtime_environment_variable(AdapterKind::CodexCli, key).as_deref(),
                    Some(std::ffi::OsStr::new(expected))
                );
                let mut other = tokio::process::Command::new("fixture-only-never-spawned");
                configure_runtime_command(AdapterKind::Pi, &mut other);
                assert!(!other.as_std().get_envs().any(|(name, _)| name == key));
            })
            .await;
        };
        tokio::join!(
            check(first, "first-private-value"),
            check(second, "second-private-value")
        );
        assert_eq!(std::env::var_os(key), inherited);
        assert_eq!(
            runtime_environment_variable(AdapterKind::CodexCli, key),
            inherited
        );
    }
}
