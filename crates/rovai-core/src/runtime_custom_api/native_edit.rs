//! Narrow native edits. A catalog is staged first; the one authoritative config is replaced last.
use super::{
    ApiKeyChange, ConnectionMode, CustomApiConfiguration, FieldEdit,
    native::{self, CredentialSource, NativeContext, NativeRead},
    native_file::NativeFile,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub fn write(
    context: &NativeContext,
    current: &NativeRead,
    desired: &CustomApiConfiguration,
    edits: &[FieldEdit],
    key: &ApiKeyChange,
    generated_catalog: Option<&Value>,
) -> Result<bool> {
    write_with_saved_environment(
        context,
        current,
        desired,
        edits,
        key,
        generated_catalog,
        context,
    )
    .map(|written| !written.is_empty())
}

/// Only Save may remove Rovai-owned environment entries. Validate official selection
/// against the environment that will remain after the same save commits.
pub(crate) fn write_with_saved_environment(
    context: &NativeContext,
    current: &NativeRead,
    desired: &CustomApiConfiguration,
    edits: &[FieldEdit],
    key: &ApiKeyChange,
    generated_catalog: Option<&Value>,
    saved_environment: &NativeContext,
) -> Result<Vec<(NativeFile, Option<Vec<u8>>)>> {
    let official = desired.mode() == Some(ConnectionMode::OfficialLogin);
    let key = if official { &ApiKeyChange::Keep } else { key };
    key.validate()?;
    let changed = |name: &str| {
        edits
            .iter()
            .any(|e| e.path.first().is_some_and(|p| p == name))
    };
    let mode_changed = edits
        .iter()
        .any(|edit| edit.path == ["mode"] && edit.before != edit.after);
    let current_api = current.configuration.mode() != Some(ConnectionMode::OfficialLogin);
    let cloud = matches!(current.source, CredentialSource::ClaudeCloud { .. });
    let replace_cloud = cloud && (changed("baseUrl") || !key.is_keep());
    ensure!(
        official || !replace_cloud || matches!(key, ApiKeyChange::Replace { .. }),
        "改用 Messages 接口时请填写该接口的 API Key；原有云厂商认证不会迁移。"
    );
    if !changed("mode")
        && !changed("baseUrl")
        && !changed("claudeModels")
        && !changed("codexModels")
        && !changed("defaultRowId")
        && key.is_keep()
    {
        return Ok(Vec::new());
    }
    ensure!(
        official || current_api || !key.is_keep() || !(changed("mode") || changed("baseUrl")),
        "请填写 API Key；官方登录凭据不能作为自定义 API 凭据迁移。"
    );
    let path = context.path();
    super::codex_source::validate_edits(context, current, desired, edits)?;
    let file = NativeFile::read(&path)?;
    let base_guard = context
        .codex_source
        .as_ref()
        .filter(|source| source.profile.is_some())
        .map(|source| NativeFile::read(&source.base))
        .transpose()?;
    // Re-read after conflict resolution and immediately before constructing the native patch.
    ensure!(
        native::read(context, current.configuration.mode())?.revision == current.revision,
        "原生配置在保存期间又发生变化；草稿已保留，请再次保存以合并最新字段。"
    );
    let mut credential_patch: CredentialPatch = None;
    let mut extra_patches: Vec<(std::path::PathBuf, Option<Vec<u8>>, Vec<u8>)> = Vec::new();
    let bytes = if official {
        official_configuration(
            context,
            current,
            &mut credential_patch,
            saved_environment,
            &mut extra_patches,
        )?
    } else {
        match desired {
            CustomApiConfiguration::ClaudeCode {
                base_url, models, ..
            } => {
                let mut doc = native::read_json(&path)?;
                if mode_changed && doc["forceLoginMethod"] == "claudeai" {
                    doc.as_object_mut().unwrap().remove("forceLoginMethod");
                }
                if doc.get("env").is_none() {
                    doc["env"] = json!({});
                }
                ensure!(doc["env"].is_object(), "Claude Code 的 env 必须是对象。");
                if replace_cloud {
                    ensure!(
                        !saved_environment
                            .env("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST")
                            .is_some_and(|v| matches!(v.as_str(), "1" | "true")),
                        "Claude 路由由外部宿主管理，个人配置无法切换；请在该宿主调整路由。草稿已保留。"
                    );
                    for (flag, _, _) in native::CLAUDE_CLOUD_ROUTES {
                        if saved_environment.env(flag).is_some_and(|v| !v.is_empty()) {
                            doc["env"][flag] = json!("0");
                        } else {
                            doc["env"].as_object_mut().unwrap().remove(flag);
                        }
                    }
                }
                if changed("baseUrl") || replace_cloud {
                    doc["env"]["ANTHROPIC_BASE_URL"] = json!(base_url);
                }
                let names = [
                    "model",
                    "reasoningModel",
                    "haikuModel",
                    "sonnetModel",
                    "opusModel",
                ];
                for ((env, value), name) in native::claude_models(models).into_iter().zip(names) {
                    if edits.iter().any(|e| e.path == ["claudeModels", name]) {
                        let env_model = doc["env"][env]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| context.env(env))
                            .unwrap_or_default();
                        if name == "model" && env_model.is_empty() && doc["model"].is_string() {
                            if value.is_empty() {
                                doc.as_object_mut().unwrap().remove("model");
                            } else {
                                doc["model"] = json!(value);
                            }
                        } else {
                            doc["env"][env] = json!(value);
                            // Clearing the effective primary model must not resurrect a shadowed one.
                            if name == "model" && value.is_empty() {
                                doc.as_object_mut().unwrap().remove("model");
                            }
                        }
                    }
                }
                match key {
                    ApiKeyChange::Keep => {}
                    ApiKeyChange::Replace { value } => {
                        let variable = current.source.claude_variable();
                        doc["env"][variable] = json!(value.trim());
                        let other = if variable == "ANTHROPIC_API_KEY" {
                            "ANTHROPIC_AUTH_TOKEN"
                        } else {
                            "ANTHROPIC_API_KEY"
                        };
                        doc["env"][other] = json!("");
                        doc["apiKeyHelper"] = json!("");
                    }
                    ApiKeyChange::Clear => {
                        ensure!(
                            current.credential.can_clear,
                            "{} 无法在此清除；请在该来源处理，或输入新 Key 替换连接。",
                            current.credential.source_label
                        );
                        doc["env"]["ANTHROPIC_AUTH_TOKEN"] = json!("");
                        doc["env"]["ANTHROPIC_API_KEY"] = json!("");
                        doc["apiKeyHelper"] = json!("");
                    }
                }
                let mut bytes = serde_json::to_vec_pretty(&doc)?;
                bytes.push(b'\n');
                bytes
            }
            CustomApiConfiguration::Codex {
                base_url,
                default_model,
                default_row_id,
                ..
            } => {
                let mut doc = codex_document(context, &mut extra_patches)?;
                if mode_changed
                    && doc
                        .get("forced_login_method")
                        .and_then(toml_edit::Item::as_str)
                        == Some("chatgpt")
                {
                    doc.as_table_mut().remove("forced_login_method");
                }
                let reuse_connection = current_api && key.is_keep();
                let connection_changed = !key.is_keep();
                let effective = native::codex_config(context)?;
                if mode_changed
                    && current.configuration.mode().is_none()
                    && matches!(current.source, CredentialSource::NativeManaged { .. })
                {
                    doc["forced_login_method"] = toml_edit::value("api");
                }
                if mode_changed
                    && effective["forced_login_method"] == "chatgpt"
                    && context
                        .codex_source
                        .as_ref()
                        .is_some_and(|s| s.profile.is_some())
                {
                    doc["forced_login_method"] = toml_edit::value("api");
                }
                let mut provider_id = if connection_changed && current.provider_id == "openai" {
                    "rovai_custom".to_owned()
                } else {
                    current.provider_id.clone()
                };
                if connection_changed
                    && context
                        .codex_source
                        .as_ref()
                        .is_some_and(|source| source.profile.is_some())
                    && current.provider_id != "openai"
                {
                    // Removing a field in the upper layer would inherit lower auth
                    // again. Copy the selected provider's non-auth options to a new
                    // upper-layer definition, then replace its auth source once.
                    let mut index = 1;
                    loop {
                        provider_id = format!("{}_rovai_{index}", current.provider_id);
                        if effective["model_providers"].get(&provider_id).is_none() {
                            break;
                        }
                        index += 1;
                    }
                    let mut provider = effective["model_providers"][&current.provider_id].clone();
                    if let Some(fields) = provider.as_object_mut() {
                        for name in ["auth", "aws", "env_key", "experimental_bearer_token"] {
                            fields.remove(name);
                        }
                    }
                    let values: toml::Value = serde_json::from_value(provider)?;
                    let fields = toml::to_string(&values)?.parse::<toml_edit::DocumentMut>()?;
                    let target = table(&mut doc, &["model_providers", &provider_id])?;
                    for (name, value) in fields.iter() {
                        target.insert(name, value.clone());
                    }
                }
                if reuse_connection && changed("baseUrl") && current.provider_id != "openai" {
                    set(
                        table(&mut doc, &["model_providers", &provider_id])?,
                        "base_url",
                        toml_edit::value(base_url),
                    );
                }
                if connection_changed {
                    let provider_path = ["model_providers", &provider_id];
                    let provider = table(&mut doc, &provider_path)?;
                    if current.provider_id == "openai" {
                        set(provider, "name", toml_edit::value("Rovai custom API"));
                    }
                    if changed("baseUrl") || current.provider_id == "openai" {
                        set(provider, "base_url", toml_edit::value(base_url));
                    }
                    set(provider, "wire_api", toml_edit::value("responses"));
                    match key {
                        ApiKeyChange::Keep => {}
                        ApiKeyChange::Replace { value } => {
                            // Native inline bearer is a supported provider source; no shell or auth.json mutation.
                            provider.remove("env_key");
                            provider.remove("auth");
                            provider.remove("aws");
                            set(
                                provider,
                                "experimental_bearer_token",
                                toml_edit::value(value.trim()),
                            );
                            set(provider, "requires_openai_auth", toml_edit::value(false));
                        }
                        ApiKeyChange::Clear => {
                            ensure!(
                                current.credential.can_clear,
                                "此凭据由原生认证管理；请在原生来源清除，或输入新 Key 替换当前连接。"
                            );
                            if let CredentialSource::Json {
                                path: auth_path,
                                pointer,
                                ..
                            } = &current.source
                            {
                                ensure!(
                                    pointer == "/OPENAI_API_KEY",
                                    "此原生凭据字段无法安全清除。"
                                );
                                let original = native::read_bytes(auth_path)?;
                                let mut auth = native::read_json(auth_path)?;
                                auth.as_object_mut()
                                    .ok_or_else(|| anyhow::anyhow!("原生认证文件格式无效。"))?
                                    .remove("OPENAI_API_KEY");
                                let mut updated = serde_json::to_vec_pretty(&auth)?;
                                updated.push(b'\n');
                                credential_patch = Some((auth_path.clone(), original, updated));
                            }
                            provider.remove("experimental_bearer_token");
                            provider.remove("auth");
                            provider.remove("aws");
                            // A required, unset reference prevents falling back to a saved official account.
                            set(
                                provider,
                                "env_key",
                                toml_edit::value("ROVAI_UNCONFIGURED_API_KEY"),
                            );
                            set(provider, "requires_openai_auth", toml_edit::value(false));
                        }
                    }
                }
                let target = codex_target(context, &mut doc)?;
                if connection_changed {
                    set(target, "model_provider", toml_edit::value(&provider_id));
                }
                if reuse_connection && changed("baseUrl") && current.provider_id == "openai" {
                    set(target, "openai_base_url", toml_edit::value(base_url));
                }
                if changed("codexModels") {
                    let catalog = generated_catalog
                        .ok_or_else(|| anyhow::anyhow!("模型目录尚未生成，原配置未被修改。"))?;
                    let hash = crate::command::canonical_json_digest(catalog)?;
                    // Native config references a key-free catalog in the native config directory.
                    let catalog_path = context
                        .native_home()
                        .join("rovai-model-catalogs")
                        .join(format!("{}.json", hash.trim_start_matches("sha256:")));
                    let contents = serde_json::to_vec(catalog)?;
                    if catalog_path.exists() {
                        ensure!(
                            std::fs::read(&catalog_path)? == contents,
                            "模型目录修订内容已变化，拒绝覆盖。"
                        );
                    } else {
                        crate::platform::private_storage::atomic_write_private_bytes(
                            &catalog_path,
                            &contents,
                        )?;
                    }
                    set(
                        target,
                        "model_catalog_json",
                        toml_edit::value(catalog_path.to_string_lossy().as_ref()),
                    );
                }
                let renamed_default = matches!(&current.configuration,
                    CustomApiConfiguration::Codex { default_row_id: old_row, default_model: old_model, .. }
                    if old_row == default_row_id && old_model != default_model);
                if changed("defaultRowId")
                    || (changed("codexModels") && renamed_default && !default_model.is_empty())
                {
                    set(target, "model", toml_edit::value(default_model));
                }
                doc.to_string().into_bytes()
            }
        }
    };
    file.unchanged()?;
    if let Some(base) = &base_guard {
        base.unchanged()?;
    }
    if let Some(patch) = credential_patch {
        extra_patches.push(patch);
    }
    let mut staged = Vec::new();
    for (path, before, after) in extra_patches {
        let target = NativeFile::read(&path)?;
        ensure!(
            target.matches(&before)?,
            "原生来源在保存期间变化，草稿已保留。"
        );
        staged.push((target, after));
    }
    staged.push((file, bytes));
    for (file, _) in &staged {
        file.unchanged()?;
    }
    let mut written: Vec<(NativeFile, Option<Vec<u8>>)> = Vec::new();
    for (file, bytes) in staged {
        if let Err(error) = file.write(&bytes) {
            for (file, after) in written.iter().rev() {
                file.restore(after)?;
            }
            anyhow::bail!("保存原生配置失败；草稿已保留：{error:#}");
        }
        written.push((file, Some(bytes)));
    }
    Ok(written)
}

type CredentialPatch = Option<(std::path::PathBuf, Option<Vec<u8>>, Vec<u8>)>;

/// Change the native selection once, on Save. Dormant providers and OAuth stay native-owned.
fn official_configuration(
    context: &NativeContext,
    current: &NativeRead,
    credential_patch: &mut CredentialPatch,
    saved_environment: &NativeContext,
    extra_patches: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>, Vec<u8>)>,
) -> Result<Vec<u8>> {
    if context.kind == crate::agent_profile::AdapterKind::ClaudeCodeCli {
        let mut doc = native::read_json(&context.path())?;
        // This document is the user's settings only. Managed policy remains
        // in native sources and keeps its native precedence.
        if doc["forceLoginMethod"] == "console" {
            doc.as_object_mut().unwrap().remove("forceLoginMethod");
        }
        if current.configuration.enabled() {
            if doc.get("env").is_none() {
                doc["env"] = json!({});
            }
            let headers = doc["env"]["ANTHROPIC_CUSTOM_HEADERS"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| context.env("ANTHROPIC_CUSTOM_HEADERS"))
                .unwrap_or_default();
            let retained_headers = headers
                .lines()
                .filter(|line| {
                    line.split_once(':').is_none_or(|(name, _)| {
                        !matches!(
                            name.trim().to_ascii_lowercase().as_str(),
                            "authorization" | "x-api-key"
                        )
                    })
                })
                .collect::<Vec<_>>()
                .join("\n");
            let env = doc["env"]
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Claude Code 的 env 必须是对象。"))?;
            for name in [
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_MODEL",
                "ANTHROPIC_REASONING_MODEL",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                "ANTHROPIC_DEFAULT_SONNET_MODEL",
                "ANTHROPIC_DEFAULT_OPUS_MODEL",
                "ANTHROPIC_PROFILE",
                "ANTHROPIC_FEDERATION_RULE_ID",
                "ANTHROPIC_ORGANIZATION_ID",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
            ] {
                // Native settings.env can mask an inherited shell value. This is
                // a saved native setting, never a per-execution environment patch.
                if saved_environment.env(name).is_some_and(|v| !v.is_empty()) {
                    env.insert(name.into(), json!(""));
                } else {
                    env.remove(name);
                }
            }
            if retained_headers != headers {
                // Remove only model authentication headers; tracing and proxy auth remain native-owned.
                env.insert("ANTHROPIC_CUSTOM_HEADERS".into(), json!(retained_headers));
            }
            let fields = doc.as_object_mut().unwrap();
            fields.remove("apiKeyHelper");
            fields.remove("model");
        }
        let mut bytes = serde_json::to_vec_pretty(&doc)?;
        bytes.push(b'\n');
        return Ok(bytes);
    }
    let mut doc = codex_document(context, extra_patches)?;
    if doc
        .get("forced_login_method")
        .and_then(toml_edit::Item::as_str)
        == Some("api")
    {
        doc.as_table_mut().remove("forced_login_method");
    }
    for name in ["OPENAI_API_KEY", "CODEX_API_KEY", "OPENAI_BASE_URL"] {
        ensure!(
            !saved_environment.env(name).is_some_and(|v| !v.is_empty()),
            "当前连接使用环境变量 {}，无法通过原生配置文件停用；请在启动该程序的环境中移除该覆盖后重试。草稿已保留。",
            name
        );
    }
    let target = codex_target(context, &mut doc)?;
    set(target, "model_provider", toml_edit::value("openai"));
    target.remove("openai_base_url");
    if current.configuration.mode() != Some(ConnectionMode::OfficialLogin) {
        target.remove("model");
        target.remove("model_catalog_json");
        // Root defaults also participate in the selected profile's fallback.
        doc.as_table_mut().remove("model");
        doc.as_table_mut().remove("model_catalog_json");
    }
    doc.as_table_mut().remove("openai_base_url");
    // Native profile deletion exposes the base layer. Remove only the related
    // lower-layer API defaults, retaining every other base/provider field.
    if let Some(source) = &context.codex_source {
        if source.profile.is_some() {
            patch_base(context, extra_patches, |base| {
                base.as_table_mut().remove("openai_base_url");
                if current.configuration.mode() != Some(ConnectionMode::OfficialLogin) {
                    base.as_table_mut().remove("model");
                    base.as_table_mut().remove("model_catalog_json");
                }
                Ok(())
            })?;
        }
    }
    let effective = native::codex_config(context)?;
    let store = effective["cli_auth_credentials_store"]
        .as_str()
        .unwrap_or("file");
    let managed_store = matches!(store, "keyring" | "auto" | "ephemeral");
    let mut cleared_api_without_oauth = false;
    if matches!(store, "file" | "auto") {
        let auth_path = context.native_home().join("auth.json");
        let before = native::read_bytes(&auth_path)?;
        let mut auth = native::read_json(&auth_path)?;
        if (auth["auth_mode"] != "chatgpt" && auth.get("OPENAI_API_KEY").is_some())
            || auth["auth_mode"] == "apikey"
        {
            let oauth = auth
                .pointer("/tokens/access_token")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty());
            let fields = auth
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Codex 原生认证文件格式无效。"))?;
            fields.remove("OPENAI_API_KEY");
            // An untyped empty auth object is interpreted as ChatGPT by the
            // native loader. Keep its API type until a real login replaces it;
            // the native selection below filters it before credential parsing.
            fields.insert(
                "auth_mode".into(),
                json!(if oauth { "chatgpt" } else { "apikey" }),
            );
            cleared_api_without_oauth = !oauth;
            let mut bytes = serde_json::to_vec_pretty(&auth)?;
            bytes.push(b'\n');
            *credential_patch = Some((auth_path, before, bytes));
        }
    }
    if managed_store
        || cleared_api_without_oauth
        || (effective["forced_login_method"] == "api"
            && context
                .codex_source
                .as_ref()
                .is_some_and(|s| s.profile.is_some()))
    {
        // Native AuthConfig filters stored API identities in this mode. Do not
        // inspect, copy or delete a keyring object (which may also contain OAuth).
        // A later explicit API selection removes this personal restriction.
        doc["forced_login_method"] = toml_edit::value("chatgpt");
    }
    Ok(doc.to_string().into_bytes())
}
fn table<'a>(
    document: &'a mut toml_edit::DocumentMut,
    keys: &[&str],
) -> Result<&'a mut dyn toml_edit::TableLike> {
    let mut current: &mut dyn toml_edit::TableLike = document.as_table_mut();
    for key in keys {
        if !current.contains_key(key) {
            current.insert(key, toml_edit::value(toml_edit::InlineTable::new()));
        }
        current = current
            .get_mut(key)
            .and_then(toml_edit::Item::as_table_like_mut)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Codex 原生字段 {} 不是可编辑配置表；请保留当前草稿并修复原生结构。",
                    key
                )
            })?;
    }
    Ok(current)
}
pub fn value_at<'a>(value: &'a Value, path: &[String]) -> &'a Value {
    path.iter()
        .fold(value, |value, name| value.get(name).unwrap_or(&Value::Null))
}
pub fn set_at(value: &mut Value, path: &[String], next: Value) -> Result<()> {
    let (first, rest) = path
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("修改字段不能为空。"))?;
    ensure!(value.is_object(), "字段所在项目已被删除，请处理字段冲突。");
    if rest.is_empty() {
        value[first] = next;
    } else {
        if value.get(first).is_none_or(Value::is_null) {
            value[first] = json!({});
        }
        set_at(&mut value[first], rest, next)?;
    }
    Ok(())
}

fn set(table: &mut dyn toml_edit::TableLike, name: &str, mut item: toml_edit::Item) {
    if let Some(old) = table.get(name).and_then(toml_edit::Item::as_value) {
        if old.to_string() == item.to_string() {
            return;
        }
        if let Some(value) = item.as_value_mut() {
            *value.decor_mut() = old.decor().clone();
        }
    }
    if let Some(existing) = table.get_mut(name) {
        *existing = item;
    } else {
        table.insert(name, item);
    }
}

fn codex_target<'a>(
    context: &NativeContext,
    doc: &'a mut toml_edit::DocumentMut,
) -> Result<&'a mut dyn toml_edit::TableLike> {
    let profile = context
        .codex_source
        .as_ref()
        .filter(|s| s.legacy)
        .and_then(|_| doc.get("profile"))
        .and_then(toml_edit::Item::as_str)
        .map(str::to_owned);
    if let Some(profile) = profile {
        table(doc, &["profiles", &profile])
    } else {
        Ok(doc.as_table_mut())
    }
}
fn patch_base(
    context: &NativeContext,
    patches: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>, Vec<u8>)>,
    change: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>,
) -> Result<()> {
    let path = &context.codex_source.as_ref().unwrap().base;
    let existing = patches.iter().position(|(p, _, _)| p == path);
    let before = native::read_bytes(path)?;
    let mut doc = if let Some(index) = existing {
        std::str::from_utf8(&patches[index].2)?.parse()?
    } else {
        native::read_toml(path)?
    };
    change(&mut doc)?;
    let after = doc.to_string().into_bytes();
    if let Some(index) = existing {
        patches[index].2 = after;
    } else if before.as_deref() != Some(after.as_slice()) {
        patches.push((path.clone(), before, after));
    }
    Ok(())
}
fn codex_document(
    context: &NativeContext,
    patches: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>, Vec<u8>)>,
) -> Result<toml_edit::DocumentMut> {
    let mut doc = native::read_toml(&context.path())?;
    if let Some(source) = &context.codex_source {
        if source.rejected_selector {
            if source.profile.is_some() {
                patch_base(context, patches, |base| {
                    base.as_table_mut().remove("profile");
                    Ok(())
                })?;
            } else {
                doc.as_table_mut().remove("profile");
            }
        }
    }
    // Dormant inline profiles are preserved in place: current Codex ignores the
    // table but rejects the selector. No credentials are copied or profiles enabled.
    Ok(doc)
}
