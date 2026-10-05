//! Local launch metadata for the native editor. Only source metadata is cached;
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
    pub field_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub selected_provider: Option<String>,
    #[serde(default)]
    pub target_unconfirmed: bool,
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
    let cached = SOURCES
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(&key(context))
        .filter(|entry| fingerprint(&entry.executable) == entry.fingerprint)
        .map(|entry| entry.source.clone());
    cached.or_else(|| {
        let executable = Path::new(context.launcher.as_deref()?);
        (needs_read(executable, context) || !executable.is_file()).then(|| Source {
            base: context.directory.join("config.toml"),
            target_unconfirmed: true,
            ..Source::default()
        })
    })
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
    is_wrapper(executable)
        || native::read_toml(&context.directory.join("config.toml"))
            .ok()
            .is_some_and(|doc| doc.get("profile").is_some())
}
fn is_wrapper(executable: &Path) -> bool {
    if executable.extension().is_some_and(|ext| {
        matches!(
            ext.to_string_lossy().to_ascii_lowercase().as_str(),
            "cmd" | "bat" | "ps1"
        )
    }) {
        return true;
    }
    let mut prefix = [0; 2];
    std::fs::File::open(executable)
        .ok()
        .is_some_and(|mut file| file.read_exact(&mut prefix).is_ok() && prefix == *b"#!")
}
/// Optional local metadata read, never a Save/execute gate or an account check.
/// Current app-server has no --profile argument. A wrapper may change CODEX_HOME;
/// use the native User layer's file rather than treating the wrapper as a binary.
pub async fn refresh(executable: &Path, context: &NativeContext, command_context: &NativeContext) {
    let mut source = context.codex_source.clone().unwrap_or_else(|| Source {
        base: context.directory.join("config.toml"),
        target_unconfirmed: is_wrapper(executable),
        ..Source::default()
    });
    source.launcher_identity = format!("{}:{}", executable.display(), fingerprint(executable));
    if !needs_read(executable, context) {
        source.target_unconfirmed = false;
        source.read_error = None;
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
                // A direct binary's directory is explicit even if an obsolete
                // selector prevents config/read. A wrapper still needs a User source.
                if !is_wrapper(executable) {
                    source.target_unconfirmed = false;
                }
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
        source.field_overrides.clear();
        source.selected_provider = value["config"]["model_provider"]
            .as_str()
            .map(str::to_owned);
        if let Some(origins) = value["origins"].as_object() {
            source.field_overrides = origins
                .iter()
                .filter_map(|(field, origin)| {
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
                    ) || field
                        .strip_prefix(&format!("model_providers.{selected}."))
                        .is_some_and(|name| {
                            matches!(
                                name,
                                "base_url"
                                    | "env_key"
                                    | "experimental_bearer_token"
                                    | "requires_openai_auth"
                                    | "auth"
                                    | "aws"
                            ) || name.starts_with("auth.")
                                || name.starts_with("aws.")
                        });
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
                    .then(|| (field.clone(), source.to_owned()))
                })
                .collect();
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
        if !files.is_empty() {
            source.target_unconfirmed = false;
        }
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
    if native_read.is_err() {
        source.read_error = Some("原生来源暂未确认，已读取的配置仍保留。".into());
    }
    remember(context, executable, source);
}
pub fn read(context: &NativeContext) -> Result<Value> {
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

fn field_label(field: &str) -> &str {
    match field {
        "model" => "默认模型",
        "model_catalog_json" => "模型列表",
        "model_provider" | "forced_login_method" | "forced_chatgpt_workspace_id" => "连接方式",
        "openai_base_url" => "接口地址",
        _ if field.ends_with(".base_url") => "接口地址",
        _ => "API 凭据或连接参数",
    }
}
fn origin_label(origin: &str) -> &str {
    if origin == "sessionFlags" {
        "启动参数"
    } else {
        "管理策略"
    }
}
pub fn observation(context: &NativeContext) -> Option<String> {
    let source = context.codex_source.as_ref()?;
    if source.target_unconfirmed {
        return Some("当前显示本地配置；实际写入目标尚未确认，连接修改暂不写入。程序路径和普通环境变量仍可保存。".into());
    }
    let mut notes = source
        .field_overrides
        .iter()
        .map(|(field, origin)| {
            format!(
                "{}由{}固定（{field}），相关修改不能在此生效",
                field_label(field),
                origin_label(origin)
            )
        })
        .collect::<Vec<_>>();
    if let Some(error) = &source.read_error {
        notes.push(error.clone());
    }
    (!notes.is_empty()).then(|| notes.join("；"))
}
/// Restrictions apply to the fields this save actually writes. They never gate
/// reading the local file or editing an unrelated field.
pub fn validate_edits(
    context: &NativeContext,
    current: &native::NativeRead,
    desired: &super::CustomApiConfiguration,
    edits: &[super::FieldEdit],
) -> Result<()> {
    let Some(source) = &context.codex_source else {
        return Ok(());
    };
    let changed = |field: &str| {
        edits
            .iter()
            .any(|e| e.path.first().is_some_and(|p| p == field))
    };
    anyhow::ensure!(
        !source.target_unconfirmed,
        "所选入口的原生写入目标尚未确认，连接修改未写入，草稿已保留。来源确认后可重试保存；程序路径和普通环境变量仍可单独保存。"
    );
    for (field, origin) in &source.field_overrides {
        let mode = changed("mode");
        let credentials = changed("credentialVersion");
        let model_changed = changed("defaultRowId")
            || (changed("codexModels")
                && match (&current.configuration, desired) {
                    (
                        super::CustomApiConfiguration::Codex {
                            default_model: a, ..
                        },
                        super::CustomApiConfiguration::Codex {
                            default_model: b, ..
                        },
                    ) => a != b,
                    _ => false,
                });
        let official = mode && desired.mode() == Some(super::ConnectionMode::OfficialLogin);
        let blocked = match field.as_str() {
            "model" => model_changed || official,
            "model_catalog_json" => changed("codexModels") || official,
            // Editing an existing provider's URL or static Key keeps its ID.
            // This is safe only when native metadata confirms that same selection.
            "model_provider" => {
                (source.selected_provider.as_deref() != Some(current.provider_id.as_str())
                    && (mode || credentials || changed("baseUrl")))
                    || (official && current.provider_id != "openai")
                    || (!official
                        && credentials
                        && (current.provider_id == "openai" || source.profile.is_some()))
            }
            "forced_login_method" | "forced_chatgpt_workspace_id" => mode || credentials,
            "openai_base_url" => changed("baseUrl") || official,
            _ if field.ends_with(".base_url") => changed("baseUrl"),
            _ => credentials,
        };
        anyhow::ensure!(
            !blocked,
            "{}由{}固定（{field}），本次相关修改无法生效；其他字段仍可编辑。草稿已保留。",
            field_label(field),
            origin_label(origin)
        );
    }
    Ok(())
}
