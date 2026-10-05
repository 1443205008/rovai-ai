//! Local launch metadata for the native editor. Only file references are cached;
//! effective values and credentials are always re-read from their native sources.
use super::native::{self, NativeContext};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub base: PathBuf,
    pub profile: Option<PathBuf>,
    /// Only an actually old CLI may interpret the inline selector.
    pub legacy: bool,
    pub rejected_selector: bool,
    #[serde(default)]
    pub launcher_identity: String,
    #[serde(default)]
    pub override_source: Option<String>,
    #[serde(default)]
    pub read_error: Option<String>,
}
#[derive(Clone)]
struct Entry {
    source: Source,
    executable: PathBuf,
    fingerprint: String,
}
static SOURCES: OnceLock<Mutex<BTreeMap<String, Entry>>> = OnceLock::new();
fn fingerprint(path: &Path) -> String {
    std::fs::metadata(path)
        .ok()
        .map(|m| format!("{}:{:?}", m.len(), m.modified().ok()))
        .unwrap_or_default()
}
fn key(context: &NativeContext) -> String {
    crate::command::canonical_json_digest(&serde_json::json!([
        context.directory,
        context.launcher,
        context.environment // digest only; no env/key values are retained in the cache
    ]))
    .unwrap_or_default()
}
pub fn resolve(context: &NativeContext) -> Option<Source> {
    SOURCES
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(&key(context))
        .filter(|entry| fingerprint(&entry.executable) == entry.fingerprint)
        .map(|entry| entry.source.clone())
}
/// Re-observe only a changed/missing launch source. Ordinary saves use captured
/// references and fresh file content, without a config or account RPC.
pub fn needs_refresh(context: &NativeContext) -> bool {
    if let Ok(sources) = SOURCES.get_or_init(Default::default).lock() {
        if let Some(entry) = sources.get(&key(context)) {
            return fingerprint(&entry.executable) != entry.fingerprint;
        }
    }
    needs_read(
        Path::new(context.launcher.as_deref().unwrap_or("")),
        context,
    )
}
fn remember(context: &NativeContext, executable: &Path, source: Source) {
    if let Ok(mut sources) = SOURCES.get_or_init(Default::default).lock() {
        if sources.len() >= 64 {
            sources.clear();
        }
        sources.insert(
            key(context),
            Entry {
                source,
                executable: executable.into(),
                fingerprint: fingerprint(executable),
            },
        );
    }
}
pub fn needs_read(executable: &Path, context: &NativeContext) -> bool {
    let mut prefix = [0; 2];
    let wrapper = std::fs::File::open(executable)
        .ok()
        .is_some_and(|mut file| file.read_exact(&mut prefix).is_ok() && prefix == *b"#!");
    wrapper
        || native::read_toml(&context.directory.join("config.toml"))
            .ok()
            .is_some_and(|doc| doc.get("profile").is_some())
}
/// Optional local metadata read, never a Save/execute gate or an account check.
/// Current app-server has no --profile argument. A wrapper may change CODEX_HOME;
/// use the native User layer's file rather than treating the wrapper as a binary.
pub async fn refresh(executable: &Path, context: &NativeContext, command_context: &NativeContext) {
    let mut source = context.codex_source.clone().unwrap_or_else(|| Source {
        base: context.directory.join("config.toml"),
        ..Source::default()
    });
    source.launcher_identity = format!("{}:{}", executable.display(), fingerprint(executable));
    if !needs_read(executable, context) {
        remember(context, executable, source);
        return;
    }
    // Read the selected entrypoint's version only to interpret a legacy selector,
    // never to deny a version or to select another installed executable.
    let mut command = tokio::process::Command::new(executable);
    command
        .env_clear()
        .envs(&command_context.environment)
        .env("CODEX_HOME", &context.directory)
        .arg("--version");
    if let Ok(output) = crate::runtime_probe_process::run_bounded_command(
        &mut command,
        crate::runtime_probe_process::ProbeCommandLimits::new(std::time::Duration::from_secs(4)),
    )
    .await
    {
        if output.status.success() && !output.stdout.truncated {
            let text = String::from_utf8_lossy(&output.stdout.bytes);
            if let Some((major, minor)) = text.split_whitespace().find_map(|part| {
                let mut numbers = part.split('.');
                Some((
                    numbers.next()?.parse::<u32>().ok()?,
                    numbers.next()?.parse::<u32>().ok()?,
                ))
            }) {
                source.legacy = major == 0 && minor < 134;
                source.rejected_selector = !source.legacy;
            }
        }
    }
    let native_read = super::codex_native::request(
        executable,
        command_context,
        "config/read",
        serde_json::json!({"includeLayers":true}),
    )
    .await;
    if let Ok(value) = &native_read {
        source.read_error = None;
        source.override_source = None;
        if let Some(origins) = value["origins"].as_object() {
            source.override_source = origins.iter().find_map(|(field, origin)| {
                let selected = value["config"]["model_provider"]
                    .as_str()
                    .unwrap_or("openai");
                let relevant = matches!(
                    field.as_str(),
                    "model"
                        | "model_provider"
                        | "openai_base_url"
                        | "model_catalog_json"
                        | "forced_login_method"
                ) || field.starts_with(&format!("model_providers.{selected}."));
                let source = origin["name"]["type"].as_str().unwrap_or_default();
                (relevant
                    && matches!(
                        source,
                        "sessionFlags"
                            | "mdm"
                            | "enterpriseManaged"
                            | "legacyManagedConfigTomlFromFile"
                            | "legacyManagedConfigTomlFromMdm"
                    ))
                .then(|| format!("{field}（{source}）"))
            });
        }
        let files: Vec<_> = value["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|layer| layer["disabledReason"].is_null() && layer["name"]["type"] == "user")
            .filter_map(|layer| {
                let file = PathBuf::from(layer["name"]["file"].as_str()?);
                file.is_absolute()
                    .then(|| (file, layer["name"]["profile"].as_str().map(str::to_owned)))
            })
            .collect();
        for (file, profile) in &files {
            if profile.is_none() {
                source.base = file.clone();
            }
        }
        // Newer/other native entrypoints may report an explicitly selected User
        // profile. Do not infer it from a legacy `profile` field in config.toml.
        source.profile = files
            .into_iter()
            .filter(|(_, profile)| profile.is_some())
            .map(|(file, _)| file)
            .next_back();
    }
    if native_read.is_err() && context.codex_source.is_none() && !source.base.exists() {
        source.read_error = Some("所选启动入口未返回原生配置来源，尚不能确定可写文件。请检查该入口的配置错误后重试；其他启动设置仍可编辑。".into());
    }
    remember(context, executable, source);
}
pub fn read(context: &NativeContext) -> Result<Value> {
    if let Some(error) = context
        .codex_source
        .as_ref()
        .and_then(|s| s.read_error.as_ref())
    {
        anyhow::bail!("{error}");
    }

    if let Some(source) = context
        .codex_source
        .as_ref()
        .and_then(|s| s.override_source.as_ref())
    {
        anyhow::bail!(
            "原生启动覆盖或管理策略控制 {source}，修改个人文件不会生效；请在该启动入口或管理来源调整。其他启动设置仍可编辑。"
        );
    }
    let base = context
        .codex_source
        .as_ref()
        .map(|s| s.base.clone())
        .unwrap_or_else(|| context.directory.join("config.toml"));
    let parse = |path: &Path| -> Result<Value> {
        let doc = native::read_toml(path)?;
        let value: toml::Value = toml::from_str(&doc.to_string())?;
        Ok(serde_json::to_value(value)?)
    };
    let mut value = parse(&base)?;
    if let Some(profile) = context
        .codex_source
        .as_ref()
        .and_then(|s| s.profile.as_ref())
    {
        merge(&mut value, parse(profile)?);
    } else if context.codex_source.as_ref().is_some_and(|s| s.legacy) {
        if let Some(profile) = value["profile"]
            .as_str()
            .and_then(|p| value["profiles"].get(p))
            .cloned()
        {
            merge(&mut value, profile);
        }
    }
    Ok(value)
}
fn merge(base: &mut Value, upper: Value) {
    if let (Some(base), Some(upper)) = (base.as_object_mut(), upper.as_object()) {
        for (key, value) in upper {
            merge(
                base.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
            );
        }
    } else {
        *base = upper;
    }
}
pub fn field_file(context: &NativeContext, keys: &[&str]) -> PathBuf {
    if let Some(source) = &context.codex_source {
        if let Some(profile) = &source.profile {
            if native::read_toml(profile).ok().is_some_and(|doc| {
                let mut value = doc.as_item();
                for key in keys {
                    let Some(next) = value.get(key) else {
                        return false;
                    };
                    value = next;
                }
                !value.is_none()
            }) {
                return profile.clone();
            }
        }
        return source.base.clone();
    }
    context.path()
}
