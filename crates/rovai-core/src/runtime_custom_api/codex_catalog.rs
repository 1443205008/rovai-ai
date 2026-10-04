//! Preserve native catalogs; new metadata comes from the actual selected entrypoint.
//! Known metadata is read from the selected executable, never another installation/cache.
use super::{CustomApiConfiguration, CustomApiSnapshot};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;
use tokio::process::Command;

const BASE_INSTRUCTIONS: &str = include_str!("codex-0.159.2-prompt.txt");

/// Mirrors models-manager/src/model_info.rs::model_info_from_slug at rust-v0.159.2.
/// These are native compatibility defaults, not claims about a relay's capabilities.
fn fallback_model(id: &str) -> Value {
    let mut model = json!({
        "slug": id, "display_name": id, "description": null,
        "default_reasoning_level": null, "supported_reasoning_levels": [],
        "shell_type": "unified_exec", "visibility": "none", "supported_in_api": true,
        "priority": 99, "additional_speed_tiers": [], "service_tiers": [],
        "default_service_tier": null, "available_access_programs": null,
        "availability_nux": null, "upgrade": null,
        "model_messages": {"instructions_template": BASE_INSTRUCTIONS},
        "include_skills_usage_instructions": false, "include_plugin_usage_instructions": false,
        "include_apps_usage_instructions": false, "supports_reasoning_summary_parameter": true,
        "default_reasoning_summary": "auto", "support_verbosity": false, "default_verbosity": null,
        "apply_patch_tool_type": null, "web_search_tool_type": "text",
        "truncation_policy": {"mode": "bytes", "limit": 10000}, "supports_image_detail_original": false
    });
    model.as_object_mut().expect("native fallback is an object").extend(json!({
        "context_window": 272000, "max_context_window": 272000,
        "auto_compact_token_limit": null, "comp_hash": null, "effective_context_window_percent": 95,
        "experimental_supported_tools": [], "input_modalities": ["text", "image"],
        "supports_search_tool": false, "supports_experimental_context": false,
        "use_responses_lite": false, "supports_reasoning_effort_updates": false,
        "guardian": null, "node_repl_auto_review_required": false, "node_repl_disabled": false,
        "auto_review_model_override": null, "model_specialty": null,
        "tool_mode": null, "multi_agent_version": null, "multi_agent_reasoning_effort": null
    }).as_object().expect("native fallback is an object").clone());
    model
}

/// Patch only editor-owned fields. Hidden capability/unknown fields and internal
/// entries retain their native values, including when an existing row changes ID.
fn adapt_catalog(
    mut catalog: Value,
    current: &CustomApiConfiguration,
    desired: &CustomApiConfiguration,
    bundled: Option<&Value>,
    inherited_catalog: bool,
) -> Result<Value> {
    let CustomApiConfiguration::Codex { models, .. } = desired else {
        anyhow::bail!("Codex 自定义 API 类型不匹配。");
    };
    let CustomApiConfiguration::Codex {
        models: previous, ..
    } = current
    else {
        anyhow::bail!("Codex 原生配置类型不匹配。");
    };
    super::native_resource::validate_catalog(&catalog)?;
    let managed_list =
        catalog["rovai_managed_model_list"] == true || model_ids_changed(current, desired);
    let native = catalog["models"].as_array_mut().unwrap();
    let original = native.clone();
    // A generated first catalog contains bundled internal entries as well. Keep
    // them, while the explicitly edited list owns what appears in the picker.
    if !inherited_catalog && managed_list {
        for item in native.iter_mut() {
            item["visibility"] = json!("hide");
        }
    }
    for row in previous
        .iter()
        .filter(|old| !models.iter().any(|m| m.row_id == old.row_id))
    {
        if let Some(item) = native.iter_mut().find(|item| item["slug"] == row.id) {
            item["visibility"] = json!("hide");
        }
    }
    for model in models {
        let previous_row = previous.iter().find(|old| old.row_id == model.row_id);
        let old_id = previous_row.map(|row| row.id.as_str()).unwrap_or(&model.id);
        let mut item = original
            .iter()
            .find(|entry| entry["slug"] == old_id)
            .cloned()
            .or_else(|| {
                original
                    .iter()
                    .find(|entry| entry["slug"] == model.id)
                    .cloned()
            })
            .or_else(|| {
                bundled
                    .and_then(|c| c["models"].as_array())
                    .and_then(|entries| entries.iter().find(|entry| entry["slug"] == model.id))
                    .cloned()
            })
            .unwrap_or_else(|| fallback_model(&model.id));
        item["slug"] = json!(model.id);
        item["visibility"] = json!("list");
        if previous_row.is_none_or(|old| old.display_name != model.display_name)
            || old_id != model.id
            || !inherited_catalog
        {
            item["display_name"] = json!(if model.display_name.is_empty() {
                &model.id
            } else {
                &model.display_name
            });
        }
        if let Some(index) = original.iter().position(|entry| entry["slug"] == old_id) {
            native[index] = item;
        } else {
            native.push(item);
        }
    }
    catalog["rovai_managed_model_list"] = json!(managed_list);
    super::native_resource::validate_catalog(&catalog)?;
    Ok(catalog)
}

pub async fn generate(
    executable: Option<&Path>,
    context: &super::native::NativeContext,
    current: &super::native::NativeRead,
    desired: &CustomApiConfiguration,
) -> Result<Value> {
    let existing = current
        .catalog_path
        .as_ref()
        .map(|p| super::native::read_json(p))
        .transpose()?;
    let CustomApiConfiguration::Codex { models, .. } = desired else {
        anyhow::bail!("Codex 连接类型不匹配。");
    };
    let previous = match &current.configuration {
        CustomApiConfiguration::Codex { models, .. } => models,
        _ => unreachable!(),
    };
    let needs_metadata = existing.is_none()
        || models.iter().any(|model| {
            let id = previous
                .iter()
                .find(|old| old.row_id == model.row_id)
                .map(|old| &old.id)
                .unwrap_or(&model.id);
            !existing
                .as_ref()
                .and_then(|c| c["models"].as_array())
                .is_some_and(|entries| entries.iter().any(|entry| entry["slug"] == *id))
        });
    let bundled = if needs_metadata {
        let executable =
            executable.context("新模型目录需要读取当前 Codex 程序的本地元数据，请先选择程序。")?;
        Some(super::native_resource::bundled_catalog(executable, context).await?)
    } else {
        None
    };
    let base = existing
        .clone()
        .or_else(|| bundled.clone())
        .context("Codex 原生目录不可用。")?;
    adapt_catalog(
        base,
        &current.configuration,
        desired,
        bundled.as_ref(),
        existing.is_some(),
    )
}

pub fn models_changed(before: &CustomApiConfiguration, after: &CustomApiConfiguration) -> bool {
    matches!((before, after), (CustomApiConfiguration::Codex { models: a, .. }, CustomApiConfiguration::Codex { models: b, .. }) if a != b)
}
pub fn model_ids_changed(before: &CustomApiConfiguration, after: &CustomApiConfiguration) -> bool {
    match (before, after) {
        (
            CustomApiConfiguration::Codex { models: a, .. },
            CustomApiConfiguration::Codex { models: b, .. },
        ) => {
            a.iter()
                .map(|m| &m.id)
                .collect::<std::collections::BTreeSet<_>>()
                != b.iter()
                    .map(|m| &m.id)
                    .collect::<std::collections::BTreeSet<_>>()
        }
        _ => false,
    }
}
pub(super) fn write_catalog(
    snapshot: &CustomApiSnapshot,
    catalog: &Value,
) -> Result<std::path::PathBuf> {
    // An unchanged connection can be launched by an updated runtime. Its new
    // bundled catalog must not overwrite (or collide with) an older process's file.
    let digest = crate::command::canonical_json_digest(catalog)?;
    snapshot.write_artifact(
        &format!(
            "codex-catalog-{}.json",
            digest.trim_start_matches("sha256:")
        ),
        &serde_json::to_vec(catalog)?,
    )
}

pub fn execution_provider(snapshot: &CustomApiSnapshot) -> Result<String> {
    if !snapshot.configuration.enabled() {
        return Ok("openai".into());
    }
    if snapshot.provider_id != "openai"
        || matches!(
            snapshot.credential_source,
            super::native::CredentialSource::NativeManaged { .. }
        )
    {
        // Keep the native provider: query parameters, transport, retry and timeout
        // settings (including future native fields) must survive execution binding.
        return Ok(snapshot.provider_id.clone());
    }
    let identity = snapshot.identity()?;
    Ok(format!(
        "rovai_custom_{}",
        identity
            .trim_start_matches("sha256:")
            .chars()
            .take(16)
            .collect::<String>()
    ))
}
pub async fn configure(snapshot: &CustomApiSnapshot, command: &mut Command) -> Result<()> {
    snapshot.assert_current()?;
    let CustomApiConfiguration::Codex {
        base_url,
        default_model,
        ..
    } = &snapshot.configuration
    else {
        anyhow::bail!("Codex 连接类型不匹配。");
    };
    let native_config = super::native::codex_config(&snapshot.context)?;
    let store = native_config["cli_auth_credentials_store"]
        .as_str()
        .unwrap_or("file");
    let auth = super::native::codex_auth_file(&snapshot.context, store)?;
    let forced = native_config["forced_login_method"].as_str();
    let file_api = auth["auth_mode"] != "chatgpt"
        && auth["OPENAI_API_KEY"]
            .as_str()
            .is_some_and(|v| !v.is_empty());
    let file_oauth = !file_api
        && auth["tokens"]["access_token"]
            .as_str()
            .is_some_and(|v| !v.is_empty());
    ensure!(
        !(forced == Some("chatgpt") && file_api) && !(forced == Some("api") && file_oauth),
        "Codex 登录方式约束与原生文件凭据冲突（auto 也可能回退到 auth.json）；已停止启动以避免原生 CLI 清除凭据，请在原生来源处理。"
    );
    let mut overrides = Vec::new();
    let provider = execution_provider(snapshot)?;
    overrides.push(("model_provider".to_string(), json!(provider)));
    if snapshot.configuration.enabled() {
        let opaque = matches!(
            snapshot.credential_source,
            super::native::CredentialSource::NativeManaged { .. }
        );
        if !opaque {
            ensure!(
                snapshot.key()?.is_some(),
                "当前 API 连接缺少可复用凭据，请填写 Key 或修复原生凭据引用。"
            );
            snapshot.configure_environment(command)?;
            let (headers, values) = native_headers(snapshot)?;
            command.envs(values);
            // Codex merges native config tables recursively. Overlay only these
            // fields on the selected provider, preserving query parameters,
            // transport, timeouts and future fields without copying their values.
            // Use a table value: -c dotted paths do not parse quoted segments.
            let mut config = json!({"base_url":base_url, "env_key":"ROVAI_CUSTOM_API_KEY", "wire_api":"responses", "requires_openai_auth":false});
            if provider != snapshot.provider_id {
                // Built-in OpenAI cannot be overwritten in its registry. Only
                // that path needs a new API provider, with no native table to lose.
                config["name"] = json!("Rovai custom API");
            }
            if !headers.is_empty() {
                config["env_http_headers"] = json!(headers);
            }
            overrides.push(("model_providers".into(), json!({ &provider: config })));
        } else if provider == "openai" {
            // Built-in providers do not appear in model_providers; their endpoint
            // override is a root setting, while credentials remain native-owned.
            overrides.push(("openai_base_url".into(), json!(base_url)));
        }
        if !default_model.is_empty() {
            overrides.push(("model".into(), json!(default_model)));
        }
        // An unchanged native catalog remains authoritative. Generation occurs only for an edited draft.
        let current = super::native::read(&snapshot.context, snapshot.configuration.mode())?;
        if models_changed(&current.configuration, &snapshot.configuration) {
            let catalog = generate(
                Some(Path::new(command.as_std().get_program())),
                &snapshot.context.for_command(command),
                &current,
                &snapshot.configuration,
            )
            .await?;
            let path = write_catalog(snapshot, &catalog)?;
            overrides.push(("model_catalog_json".into(), json!(path)));
        }
    } else {
        // forced_login_method can delete incompatible auth.json. Never invoke it for switching.
        ensure!(
            store != "file" || !file_api,
            "Codex 原生认证文件仍使用 API Key，此版本无法在不改写该凭据的情况下切到官方登录。请先在原生配置中处理认证来源；Rovai 未删除登录信息。"
        );
        for name in [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "OPENAI_BASE_URL",
        ] {
            command.env_remove(name);
        }
        overrides.push((
            "openai_base_url".into(),
            json!("https://chatgpt.com/backend-api/codex"),
        ));
        let native = super::native::read(&snapshot.context, snapshot.configuration.mode())?;
        if native.observation.initial_mode == Some(super::ConnectionMode::CustomApi) {
            let catalog = super::native_resource::bundled_catalog(
                Path::new(command.as_std().get_program()),
                &snapshot.context.for_command(command),
            )
            .await?;
            let model = catalog["models"]
                .as_array()
                .and_then(|models| {
                    models
                        .iter()
                        .filter(|model| model["visibility"] == "list")
                        .min_by_key(|model| model["priority"].as_i64().unwrap_or(i64::MAX))
                })
                .and_then(|model| model["slug"].as_str())
                .context("原生目录没有可用默认模型。")?;
            let path = write_catalog(snapshot, &catalog)?;
            overrides.push(("model_catalog_json".into(), json!(path)));
            overrides.push(("model".into(), json!(model)));
        }
    }
    for (key, value) in overrides {
        command
            .arg("-c")
            .arg(format!("{key}={}", toml_value(&value)?));
    }
    Ok(())
}

/// config/read is local native state, not a request to the configured API.
pub fn validate_effective(snapshot: &CustomApiSnapshot, response: &Value) -> Result<()> {
    let config = response.get("config").context("Codex 未报告最终配置。")?;
    let provider_id = execution_provider(snapshot)?;
    ensure!(
        config["model_provider"].as_str() == Some(provider_id.as_str()),
        "Codex 的有效连接被原生设置或组织策略覆盖。"
    );
    if !snapshot.configuration.enabled() {
        ensure!(
            config["openai_base_url"] == "https://chatgpt.com/backend-api/codex",
            "Codex 官方登录端点仍被其他配置覆盖。"
        );
        return Ok(());
    }
    let provider = &config["model_providers"][&provider_id];
    if provider_id == "openai" {
        ensure!(
            config["openai_base_url"].as_str() == Some(snapshot.configuration.base_url()),
            "Codex 的有效接口地址与当前原生连接不一致。"
        );
        return Ok(());
    }
    ensure!(
        provider["base_url"].as_str() == Some(snapshot.configuration.base_url())
            && provider["wire_api"] == "responses",
        "Codex 的有效接口或 Responses 协议与本次配置不一致。"
    );
    if !matches!(
        snapshot.credential_source,
        super::native::CredentialSource::NativeManaged { .. }
    ) {
        ensure!(
            provider["env_key"] == "ROVAI_CUSTOM_API_KEY"
                && provider["requires_openai_auth"] == false,
            "Codex 的认证引用与本次配置不一致。"
        );
        let (headers, _) = native_headers(snapshot)?;
        ensure!(
            if headers.is_empty() {
                provider["env_http_headers"].is_null()
                    || provider["env_http_headers"]
                        .as_object()
                        .is_some_and(|v| v.is_empty())
            } else {
                provider["env_http_headers"] == json!(headers)
            },
            "Codex 原生请求头引用与当前连接不一致。"
        );
        let native = super::native::codex_config(&snapshot.context)?;
        let original = &native["model_providers"][&snapshot.provider_id];
        // Compare retained native fields, including query and transport settings.
        // Diagnostic output may have scrubbed the current credential already.
        if provider_id == snapshot.provider_id {
            let redactor = snapshot.redactor()?;
            for (name, expected) in original.as_object().into_iter().flatten() {
                if matches!(
                    name.as_str(),
                    "base_url"
                        | "env_key"
                        | "wire_api"
                        | "requires_openai_auth"
                        | "env_http_headers"
                ) {
                    continue;
                }
                // Unknown native fields may not be projected by config/read on
                // older versions. They remain in the original provider table.
                if provider.get(name).is_none() {
                    continue;
                }
                let mut expected = expected.clone();
                let mut actual = provider[name].clone();
                redactor.value(&mut expected);
                redactor.value(&mut actual);
                ensure!(
                    actual == expected,
                    "Codex 原生 provider 的 {} 被其他配置覆盖。",
                    name
                );
            }
        } else {
            ensure!(
                provider["experimental_bearer_token"].is_null()
                    && provider["http_headers"].is_null(),
                "Codex 当前连接出现其他认证来源，请解决原生配置冲突。"
            );
        }
    }
    Ok(())
}

/// Preserve current provider headers without putting literal header values into argv or derived files.
fn native_headers(
    snapshot: &CustomApiSnapshot,
) -> Result<(
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, String>,
)> {
    let mut headers = std::collections::BTreeMap::new();
    let mut environment = std::collections::BTreeMap::new();
    let doc = super::native::read_toml(&snapshot.context.path())?;
    let Some(provider) = doc
        .get("model_providers")
        .and_then(|p| p.get(&snapshot.provider_id))
        .and_then(toml_edit::Item::as_table_like)
    else {
        return Ok((headers, environment));
    };
    // Native skips unset environment headers. Keep those references too: their
    // presence is valid configuration, not evidence of an extra credential.
    if let Some(table) = provider
        .get("env_http_headers")
        .and_then(toml_edit::Item::as_table_like)
    {
        for (name, item) in table.iter() {
            if let Some(variable) = item.as_str() {
                headers.insert(name.to_owned(), variable.to_owned());
            }
        }
    }
    let mut values = std::collections::BTreeMap::new();
    for field in ["http_headers", "env_http_headers"] {
        if let Some(table) = provider.get(field).and_then(toml_edit::Item::as_table_like) {
            for (name, item) in table.iter() {
                if let Some(value) = item.as_str().and_then(|v| {
                    if field == "env_http_headers" {
                        snapshot.context.env(v)
                    } else {
                        Some(v.to_owned())
                    }
                }) {
                    values.insert(name.to_owned(), value);
                }
            }
        }
    }
    let key = snapshot.key()?;
    for (index, (name, value)) in values.into_iter().enumerate() {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "x-api-key" | "api-key"
        ) {
            ensure!(
                key.as_ref()
                    .is_some_and(|key| value == *key || value == format!("Bearer {key}")),
                "原生 provider 的认证请求头与当前 Key 不一致；请在该原生来源解决冲突，未尝试备用凭据。"
            );
        }
        let variable = format!("ROVAI_CUSTOM_HEADER_{index}");
        headers.insert(name, variable.clone());
        environment.insert(variable, value);
    }
    Ok((headers, environment))
}

pub fn requires_account_check(snapshot: &CustomApiSnapshot) -> bool {
    !snapshot.configuration.enabled()
        || matches!(
            snapshot.credential_source,
            super::native::CredentialSource::NativeManaged { .. }
        )
}
/// Local account metadata only; no token refresh, HTTP probe or credential extraction.
pub fn validate_account(snapshot: &CustomApiSnapshot, account: &Value) -> Result<()> {
    if snapshot.configuration.enabled() {
        ensure!(
            matches!(
                account.pointer("/account/type").and_then(Value::as_str),
                Some("apiKey" | "chatgpt")
            ),
            "当前原生凭据来源不可用；请修复该来源或为此连接输入新 Key，未尝试备用账号。"
        );
    } else {
        ensure!(
            account.pointer("/account/type").and_then(Value::as_str) == Some("chatgpt"),
            "尚未使用 ChatGPT 登录。请在本机终端运行 codex login，按提示完成登录；未使用其他 Key。"
        );
    }
    Ok(())
}

fn toml_value(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(serde_json::to_string(value)?),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Object(values) => Ok(format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{} = {}",
                    serde_json::to_string(key)?,
                    toml_value(value)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        )),
        _ => anyhow::bail!("Codex 连接配置包含不支持的值。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_catalog_preserves_internal_entries_and_unknown_ids_use_only_native_defaults() {
        let config = CustomApiConfiguration::Codex {
            mode: Some(super::super::ConnectionMode::CustomApi),
            base_url: "https://relay.example/prefix".into(),
            models: vec![
                super::super::CustomApiModel {
                    row_id: "one".into(),
                    id: "known".into(),
                    display_name: "Developer".into(),
                },
                super::super::CustomApiModel {
                    row_id: "two".into(),
                    id: "known-but-unknown-suffix".into(),
                    display_name: String::new(),
                },
            ],
            default_row_id: Some("two".into()),
            default_model: "known-but-unknown-suffix".into(),
        };
        let mut known = fallback_model("known");
        known["context_window"] = json!(123456);
        known["supports_search_tool"] = json!(true);
        let native = json!({"models":[known, fallback_model("native-internal")]});
        let mut inherited = config.clone();
        if let CustomApiConfiguration::Codex { models, .. } = &mut inherited {
            models.clear();
        }
        let catalog = adapt_catalog(native, &inherited, &config, None, false).unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0]["context_window"], 123456);
        assert_eq!(models[0]["display_name"], "Developer");
        assert_eq!(models[1]["slug"], "native-internal");
        assert_eq!(models[1]["visibility"], "hide");
        assert_eq!(models[2]["supports_search_tool"], false);
        assert_eq!(models[2]["context_window"], 272000);
        assert_eq!(models[2]["priority"], 99);
        assert_eq!(models[2]["display_name"], "known-but-unknown-suffix");
        assert!(
            adapt_catalog(json!({"data":[]}), &config, &config, None, false).is_err(),
            "model/list is not a full native catalog"
        );
        let mut original = catalog.clone();
        original["vendor_extension"] = json!({"preserved": true});
        original["models"][0]["context_window"] = json!(8192);
        original["models"][0]["input_modalities"] = json!(["text"]);
        original["models"][0]["supports_reasoning_summary_parameter"] = json!(false);
        original["models"][0]["future_capability"] = json!({"declared": false});
        original["models"][0]["upgrade"] = json!({"model": "native-internal"});
        let mut renamed = config.clone();
        if let CustomApiConfiguration::Codex { models, .. } = &mut renamed {
            models[0].display_name = "Renamed".into();
        }
        let updated = adapt_catalog(original.clone(), &config, &renamed, None, true).unwrap();
        let mut expected = original.clone();
        expected["models"][0]["display_name"] = json!("Renamed");
        assert_eq!(
            updated, expected,
            "a label edit must not rewrite capabilities or unknown fields"
        );
        if let CustomApiConfiguration::Codex { models, .. } = &mut renamed {
            models[0].id = "custom-renamed-id".into();
        }
        let updated = adapt_catalog(original.clone(), &config, &renamed, None, true).unwrap();
        expected["models"][0]["slug"] = json!("custom-renamed-id");
        assert_eq!(
            updated, expected,
            "row identity retains native metadata when its ID changes"
        );
        let mut removed = renamed.clone();
        if let CustomApiConfiguration::Codex { models, .. } = &mut removed {
            models.remove(0);
        }
        let updated = adapt_catalog(updated, &renamed, &removed, None, true).unwrap();
        assert_eq!(updated["models"][0]["visibility"], "hide");
        assert_eq!(
            updated["models"][1], original["models"][1],
            "internal entry stays byte-for-value intact"
        );
        let provider = json!({"name":"Rovai custom API", "base_url":"https://relay.example/a?b=quoted", "env_key":"ROVAI_CUSTOM_API_KEY", "wire_api":"responses", "requires_openai_auth":false});
        let encoded = toml_value(&provider).unwrap();
        let parsed: toml::Value = toml::from_str(&format!("provider={encoded}")).unwrap();
        assert_eq!(
            parsed["provider"]["base_url"].as_str(),
            Some("https://relay.example/a?b=quoted")
        );
        let snapshot = CustomApiSnapshot {
            configured_model_ids: None,
            configuration: config,
            native_revision: "fixture".into(),
            credential_version: "fixture".into(),
            provider_id: "openai".into(),
            explicit_mode: true,
            draft_key: None,
            preview: false,
            credential_source: super::super::native::CredentialSource::Missing,
            context: super::super::native::NativeContext {
                kind: crate::agent_profile::AdapterKind::CodexCli,
                directory: "/not-read".into(),
                artifact_root: "/not-read".into(),
                environment: Default::default(),
            },
        };
        let id = execution_provider(&snapshot).unwrap();
        let mut effective =
            json!({"config":{"model_provider":id,"model_providers":{&id:provider}}});
        effective["config"]["model_providers"][&id]["base_url"] =
            json!("https://relay.example/prefix");
        validate_effective(&snapshot, &effective).unwrap();
        for field in ["experimental_bearer_token", "http_headers"] {
            let mut conflict = effective.clone();
            conflict["config"]["model_providers"][&id][field] = json!("old-secret");
            let error = validate_effective(&snapshot, &conflict)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("old-secret"));
        }
        effective["config"]["model_provider"] = json!("old-provider");
        assert!(validate_effective(&snapshot, &effective).is_err());
    }
}
