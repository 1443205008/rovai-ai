//! Grok's API-key endpoint override; no Home replacement or model registration.
use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use tokio::process::Command;
use crate::{agent_profile::AdapterKind, command::canonical_json_digest, runtime_discovery::{runtime_environment_variable, runtime_home_directory}};
use super::{CustomApiSnapshot, native_resource};

const CATALOG_HASH: &str = "9d6924ec760a94f91902f60adb8bcf2cd9d3ab86891a1c83e8c01a092e56490a";
const AUXILIARY: [(&str, &str); 3] = [("web_search", "GROK_WEB_SEARCH_MODEL"), ("session_summary", "GROK_SESSION_SUMMARY_MODEL"), ("image_description", "GROK_IMAGE_DESCRIPTION_MODEL")];

fn home() -> Result<PathBuf> {
    Ok(runtime_environment_variable(AdapterKind::GrokBuild, "GROK_HOME").filter(|value| !value.is_empty()).map(PathBuf::from)
        .or_else(|| runtime_home_directory(AdapterKind::GrokBuild).map(|root| root.join(".grok")))
        .context("Grok Home 不可用。")?)
}

fn read_config(path: &Path) -> Result<toml::Value> {
    match std::fs::read_to_string(path) {
        Ok(contents) => toml::from_str(&contents).map_err(|_| anyhow::anyhow!("Grok 原生配置无法解析：{}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(toml::Value::Table(Default::default())),
        Err(_) => anyhow::bail!("Grok 原生配置无法读取：{}", path.display()),
    }
}

/// Only the selected model and its auxiliary routes participate. Values are never public.
fn resolve(snapshot: &CustomApiSnapshot, executable: &Path, selected: Option<&str>, root: &Path) -> Result<(BTreeMap<String, String>, String)> {
    let native = native_resource::embedded_json(executable, b"{\n  \"default\":", CATALOG_HASH, "Grok Build 1.0.44")?;
    let mut sources = vec![PathBuf::from("/etc/grok/managed_config.toml"), root.join("managed_config.toml"), root.join("config.toml")];
    let mut configs = sources.iter().map(|path| read_config(path)).collect::<Result<Vec<_>>>()?;
    // Native requirements can pin auxiliary routes above env. Do not override or
    // attempt to reinterpret signed/MDM policy as ordinary user configuration.
    for path in [root.join("requirements.toml"), PathBuf::from("/etc/grok/requirements.toml")] {
        ensure!(!path.exists(), "当前 Grok 自定义 API 路径无法可靠确认 requirements 策略下的连接；保留策略并停止本次覆盖：{}", path.display());
    }
    #[cfg(target_os = "macos")]
    ensure!(!has_managed_requirements()?,
        "当前 Grok 自定义 API 路径无法可靠确认 MDM 策略下的连接，未覆盖组织策略。");
    // The native override layer accepts the global models block (including headers
    // and auxiliary pins). Read only these configuration sources, never account stores.
    for name in ["GROK_CONFIG", "GROK_CONFIG_PATH"] {
        if let Some(value) = runtime_environment_variable(AdapterKind::GrokBuild, name).filter(|value| !value.is_empty()) {
            let text = if name == "GROK_CONFIG_PATH" { std::fs::read_to_string(Path::new(&value)).context("Grok 原生覆盖配置不可读。")? }
                else { value.into_string().map_err(|_| anyhow::anyhow!("Grok 原生覆盖配置无效。"))? };
            let parsed = serde_json::from_str::<Value>(&text).ok().and_then(|value| toml::Value::try_from(value).ok())
                .or_else(|| toml::from_str::<toml::Value>(&text).ok()).context("Grok 原生覆盖配置无法解析。")?;
            sources.push(PathBuf::from(name)); configs.push(parsed);
        }
    }
    for config in &configs {
        ensure!(config.get("version_overrides").is_none() && config.get("campaigns").is_none(),
            "当前 Grok 原生配置含条件覆盖，无法可靠确定本次模型路由；请先解决该配置冲突。");
    }
    let model = selected.or_else(|| snapshot.configuration.default_model()).context("Grok 默认模型缺失。")?;
    let mut selected_models = BTreeSet::from([model.to_owned()]);
    let mut environment = snapshot.environment()?;
    for (field, variable) in AUXILIARY {
        let selected = runtime_environment_variable(AdapterKind::GrokBuild, variable).and_then(|value| value.into_string().ok())
            .filter(|value| !value.trim().is_empty())
            .or_else(|| configs.iter().rev().find_map(|config| config.get("models")?.get(field)?.as_str().map(str::to_owned)))
            .or_else(|| native[field].as_str().map(str::to_owned)).context("Grok 原生目录缺少辅助模型。");
        let selected = selected?;
        selected_models.insert(selected.clone());
        environment.insert(variable.to_owned(), selected);
    }
    // Headless native suggestions can make a separate model call too. Pin their native
    // selection to the current model when no user pin exists, keeping one connection.
    let suggestion = runtime_environment_variable(AdapterKind::GrokBuild, "GROK_PROMPT_SUGGESTIONS_MODEL").and_then(|value| value.into_string().ok())
        .filter(|value| !value.trim().is_empty()).or_else(|| configs.iter().rev().find_map(|config| config.get("models")?.get("prompt_suggestion")?.as_str().map(str::to_owned)))
        .unwrap_or_else(|| model.to_owned());
    environment.insert("GROK_PROMPT_SUGGESTIONS_MODEL".into(), suggestion.clone());
    selected_models.insert(suggestion);
    let key = snapshot.key()?;
    let mut relevant = Vec::new();
    for id in &selected_models {
        let builtin = native["models"].as_array().context("Grok 原生目录无效。")?.iter()
            .any(|row| row.get("id").or_else(|| row.get("model")).and_then(Value::as_str) == Some(id));
        let mut registered = builtin;
        for (source, config) in sources.iter().zip(&configs) {
            let headers = config.get("models").and_then(|models| models.get("extra_headers"));
            ensure!(headers.is_none_or(|headers| headers.as_table().is_some_and(|headers| headers.is_empty())),
                "Grok 全局模型请求头可能覆盖认证，请在原生配置中明确解决冲突：{}", source.display());
            let Some(entry) = config.get("model").and_then(|models| models.get(id)) else { continue; };
            registered = true;
            let provider = entry.get("model_provider").and_then(toml::Value::as_str).map(|provider|
                config.get("model_providers").and_then(|providers| providers.get(provider))
                    .context("Grok 当前模型引用的 provider 无法在当前配置中确定。")
            ).transpose()?;
            for route in provider.into_iter().chain(std::iter::once(entry)) {
                validate_route(route, snapshot.configuration.base_url(), &key, &mut environment)?;
                relevant.push(json!({"source":source, "model":id, "route":route}));
            }
        }
        ensure!(registered, "Grok 原生目录未注册模型 {}；本期三字段覆盖不创建任意协议模型。", id);
    }
    // A digest, never the literal native values, enters compatibility evidence.
    let digest = canonical_json_digest(&json!({"home":root,"native":CATALOG_HASH,"routes":relevant,"models":selected_models}))?;
    Ok((environment, digest))
}

fn validate_route(route: &toml::Value, base_url: &str, key: &str, environment: &mut BTreeMap<String, String>) -> Result<()> {
    for field in ["base_url", "api_base_url"] {
        if let Some(value) = route.get(field).and_then(toml::Value::as_str) {
            ensure!(value.trim_end_matches('/') == base_url.trim_end_matches('/'), "Grok 模型级 {} 覆盖了本次接口地址；请先解决原生配置冲突。", field);
        }
    }
    if let Some(value) = route.get("api_key").and_then(toml::Value::as_str).filter(|value| !value.is_empty()) {
        ensure!(value == key, "Grok 当前模型的内联 api_key 优先于本次 Key；无法通过三字段覆盖解决，请修改原生模型配置。");
        ensure!(route.get("base_url").and_then(toml::Value::as_str).is_some(), "Grok 模型内联凭据改变了端点解析路径，无法确定本次地址。");
    }
    ensure!(route.get("auth_provider").is_none() && route.get("auth").is_none() && route.get("mtls_cert_dir").is_none(), "Grok 当前模型使用原生认证 helper，无法确定本次 Key，未尝试其他账号。");
    for field in ["extra_headers", "http_headers", "env_http_headers", "query_params"] {
        ensure!(route.get(field).is_none_or(|value| value.as_table().is_some_and(|table| table.is_empty())), "Grok 当前模型的 {} 可能覆盖认证，需先解决原生配置冲突。", field);
    }
    if let Some(value) = route.get("env_key") {
        ensure!(route.get("base_url").and_then(toml::Value::as_str).is_some(), "Grok 模型 env_key 会使用模型级端点，请在原生配置中明确与本次地址一致的 base_url。");
        let names = if let Some(name) = value.as_str() { vec![name] } else {
            value.as_array().context("Grok 模型 env_key 无效。")?.iter().map(|value| value.as_str().context("Grok 模型 env_key 无效。")).collect::<Result<Vec<_>>>()?
        };
        for name in names {
            ensure!(!name.is_empty() && name.bytes().enumerate().all(|(i,c)| c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())), "Grok 模型 env_key 无效。");
            ensure!(!matches!(name, "HOME" | "PATH" | "USERPROFILE" | "SHELL" | "ENV" | "BASH_ENV" | "ZDOTDIR") && !name.starts_with("ROVAI_") && (!name.starts_with("GROK_") || name == "GROK_CODE_XAI_API_KEY") && !name.starts_with("LD_") && !name.starts_with("DYLD_"), "Grok 模型 env_key 与受保护的进程配置冲突。");
            environment.insert(name.to_owned(), key.to_owned());
        }
    }
    Ok(())
}

pub fn configure(snapshot: &CustomApiSnapshot, command: &mut Command, selected: Option<&str>) -> Result<()> {
    let root = command.as_std().get_envs().find(|(key, _)| *key == "GROK_HOME")
        .and_then(|(_, value)| value.map(PathBuf::from)).map(Ok).unwrap_or_else(home)?;
    let (environment, _) = resolve(snapshot, Path::new(command.as_std().get_program()), selected, &root)?;
    // The legacy .env loader is deliberately not called; all current route key names
    // come from this connection. Native configuration, Skills, MCP and sessions stay put.
    for name in ["GROK_CODE_XAI_API_KEY", "GROK_MODELS_BASE_URL", "GROK_MODELS_LIST_URL"] { command.env_remove(name); }
    command.envs(environment).env("GROK_DISABLE_AUTOUPDATER", "1");
    Ok(())
}

pub fn compatibility(snapshot: &CustomApiSnapshot, executable: &Path, selected: Option<&str>) -> Result<String> {
    Ok(resolve(snapshot, executable, selected, &home()?)?.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_native_routes_cannot_steal_the_endpoint_or_credentials() {
        let url = "https://relay.example/prefix";
        let mut environment = BTreeMap::new();
        for route in [
            "base_url='https://old.example'", "api_base_url='https://old.example'", "api_key='old-key'",
            "auth_provider='login-helper'", "auth={command='helper'}", "mtls_cert_dir='/old-account'",
            "extra_headers={Authorization='Bearer old-key'}", "env_http_headers={Authorization='OLD_TOKEN'}",
            "env_key='TOKEN'", "base_url='https://relay.example/prefix'\nenv_key='GROK_XAI_API_BASE_URL'",
        ] {
            let parsed: toml::Value = toml::from_str(route).unwrap();
            let error = validate_route(&parsed, url, "new-key", &mut environment).unwrap_err().to_string();
            assert!(!error.contains("old-key"));
        }
        let parsed: toml::Value = toml::from_str("base_url='https://relay.example/prefix'\nenv_key=['TOKEN_ONE','TOKEN_TWO']").unwrap();
        validate_route(&parsed, url, "new-key", &mut environment).unwrap();
        assert_eq!(environment.get("TOKEN_ONE").map(String::as_str), Some("new-key"));
        assert_eq!(environment.get("TOKEN_TWO").map(String::as_str), Some("new-key"));
    }
}


#[cfg(target_os = "macos")]
fn has_managed_requirements() -> Result<bool> {
    // Same forced preference/domain as Grok's native policy loader. Query only
    // whether the policy exists; never decode or reinterpret organization policy.
    use std::ffi::{c_char, c_void};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
        fn CFPreferencesAppValueIsForced(key: *const c_void, application: *const c_void) -> u8;
        fn CFRelease(value: *const c_void);
    }
    // SAFETY: static NUL-terminated UTF-8 names, default allocator, retained CF
    // strings are checked before use and released exactly once on every path.
    unsafe {
        let key = CFStringCreateWithCString(std::ptr::null(), c"requirements_toml_base64".as_ptr(), 0x08000100);
        let domain = CFStringCreateWithCString(std::ptr::null(), c"ai.x.grok".as_ptr(), 0x08000100);
        if key.is_null() || domain.is_null() {
            if !key.is_null() { CFRelease(key); }
            if !domain.is_null() { CFRelease(domain); }
            anyhow::bail!("无法检查 Grok 原生组织策略，未启用自定义覆盖。");
        }
        let forced = CFPreferencesAppValueIsForced(key, domain) != 0;
        CFRelease(key); CFRelease(domain);
        Ok(forced)
    }
}
