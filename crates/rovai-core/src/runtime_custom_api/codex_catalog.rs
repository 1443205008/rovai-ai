//! Native catalog adaptation, qualified against Codex 0.159.2.
//! Known metadata is read from the selected executable, never another installation/cache.
use std::path::Path;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use tokio::process::Command;
use super::{CustomApiConfiguration, CustomApiSnapshot};

const CATALOG_MARKER: &[u8] = b"{\n  \"models\": [";
// Exact embedded resource in upstream rust-v0.159.2, excluding its trailing newline.
const CATALOG_SHA256: &str = "719c75b77ed02c783263f8fe62532ecd0abc1d1ccf2c34b12e5d221d509995a1";
const BASE_INSTRUCTIONS: &str = include_str!("codex-0.159.2-prompt.txt");

fn embedded_catalog(executable: &Path) -> Result<Value> {
    super::native_resource::embedded_json(executable, CATALOG_MARKER, CATALOG_SHA256, "Codex 0.159.2")
}

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

fn adapt_catalog(mut catalog: Value, configuration: &CustomApiConfiguration) -> Result<Value> {
    let CustomApiConfiguration::Codex { models, default_model, .. } = configuration else {
        anyhow::bail!("Codex 自定义 API 类型不匹配。");
    };
    let native = catalog.get_mut("models").and_then(Value::as_array_mut).context("Codex 原生目录缺少模型数组。")?;
    let original = native.clone();
    // Keep all native internal entries, but expose only the user's configured models.
    for item in native.iter_mut() { item["visibility"] = json!("hide"); }
    for (index, model) in models.iter().enumerate() {
        let mut item = original.iter().find(|item| item["slug"].as_str() == Some(&model.id))
            .cloned().unwrap_or_else(|| fallback_model(&model.id));
        item["visibility"] = json!("list");
        item["priority"] = json!(if &model.id == default_model { 0 } else { index + 1 });
        item["display_name"] = json!(if model.display_name.is_empty() { &model.id } else { &model.display_name });
        // A configured model must not advertise an unconfigured upgrade target in the picker.
        item["upgrade"] = Value::Null;
        if let Some(existing) = native.iter_mut().find(|entry| entry["slug"] == item["slug"]) { *existing = item; }
        else { native.push(item); }
    }
    Ok(catalog)
}

pub fn configure(snapshot: &CustomApiSnapshot, command: &mut Command) -> Result<()> {
    let CustomApiConfiguration::Codex { base_url, default_model, .. } = &snapshot.configuration else {
        anyhow::bail!("Codex 自定义 API 类型不匹配。");
    };
    let catalog = adapt_catalog(embedded_catalog(Path::new(command.as_std().get_program()))?, &snapshot.configuration)?;
    let path = snapshot.write_artifact("codex-catalog.json", &serde_json::to_vec(&catalog)?)?;
    snapshot.configure_environment(command)?;
    for (key, value) in [
        ("model_provider", json!("rovai_custom")),
        ("model_providers.rovai_custom", json!({"name":"Rovai custom API", "base_url":base_url,
            "env_key":"ROVAI_CUSTOM_API_KEY", "wire_api":"responses", "requires_openai_auth":false})),
        ("model", json!(default_model)),
        ("model_catalog_json", json!(path)),
    ] {
        let value = toml_value(&value)?;
        command.arg("-c").arg(format!("{key}={value}"));
    }
    Ok(())
}

/// config/read is local native state, not a request to the configured API.
pub fn validate_effective(snapshot: &CustomApiSnapshot, response: &Value) -> Result<()> {
    let config = response.get("config").context("Codex 未报告最终配置。")?;
    ensure!(config["model_provider"] == "rovai_custom", "Codex 的有效连接被原生设置或组织策略覆盖。");
    let provider = &config["model_providers"]["rovai_custom"];
    ensure!(provider["base_url"].as_str() == Some(snapshot.configuration.base_url())
        && provider["env_key"] == "ROVAI_CUSTOM_API_KEY"
        && provider["wire_api"] == "responses"
        && provider["requires_openai_auth"] == false,
        "Codex 的有效接口、认证引用或 Responses 协议与本次配置不一致。");
    for name in ["experimental_bearer_token", "http_headers", "env_http_headers", "api_key"] {
        let value = &provider[name];
        ensure!(value.is_null() || value.as_str() == Some("") || value.as_object().is_some_and(|value| value.is_empty()),
            "Codex 原生连接仍包含优先认证字段 {}，请解决配置冲突。", name);
    }
    Ok(())
}

fn toml_value(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(serde_json::to_string(value)?),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Object(values) => Ok(format!("{{{}}}", values.iter().map(|(key, value)|
            Ok(format!("{} = {}", serde_json::to_string(key)?, toml_value(value)?)))
            .collect::<Result<Vec<_>>>()?.join(", "))),
        _ => anyhow::bail!("Codex 连接配置包含不支持的值。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_catalog_preserves_internal_entries_and_unknown_ids_use_only_native_defaults() {
        let config = CustomApiConfiguration::Codex { enabled: true, base_url: "https://relay.example/prefix".into(),
            models: vec![super::super::CustomApiModel { id: "known".into(), display_name: "Developer".into() }, super::super::CustomApiModel { id: "known-but-unknown-suffix".into(), display_name: String::new() }], default_model: "known-but-unknown-suffix".into() };
        let mut known = fallback_model("known"); known["context_window"] = json!(123456); known["supports_search_tool"] = json!(true);
        let native = json!({"models":[known, fallback_model("native-internal")]});
        let catalog = adapt_catalog(native, &config).unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0]["context_window"], 123456);
        assert_eq!(models[0]["display_name"], "Developer");
        assert_eq!(models[1]["slug"], "native-internal");
        assert_eq!(models[1]["visibility"], "hide");
        assert_eq!(models[2]["supports_search_tool"], false);
        assert_eq!(models[2]["context_window"], 272000);
        assert_eq!(models[2]["priority"], 0);
        assert_eq!(models[2]["display_name"], "known-but-unknown-suffix");
        assert!(adapt_catalog(json!({"data":[]}), &config).is_err(), "model/list is not a full native catalog");
        let provider = json!({"name":"Rovai custom API", "base_url":"https://relay.example/a?b=quoted", "env_key":"ROVAI_CUSTOM_API_KEY", "wire_api":"responses", "requires_openai_auth":false});
        let encoded = toml_value(&provider).unwrap();
        let parsed: toml::Value = toml::from_str(&format!("provider={encoded}")).unwrap();
        assert_eq!(parsed["provider"]["base_url"].as_str(), Some("https://relay.example/a?b=quoted"));
        let snapshot = CustomApiSnapshot { configuration: config, revision: 1, credential_version: uuid::Uuid::new_v4().to_string(), storage_root: "/not-read".into() };
        let mut effective = json!({"config":{"model_provider":"rovai_custom","model_providers":{"rovai_custom":provider}}});
        effective["config"]["model_providers"]["rovai_custom"]["base_url"] = json!("https://relay.example/prefix");
        validate_effective(&snapshot, &effective).unwrap();
        for field in ["experimental_bearer_token","http_headers","env_http_headers"] {
            let mut conflict = effective.clone(); conflict["config"]["model_providers"]["rovai_custom"][field] = json!("old-secret");
            let error = validate_effective(&snapshot, &conflict).unwrap_err().to_string();
            assert!(!error.contains("old-secret"));
        }
        effective["config"]["model_provider"] = json!("old-provider");
        assert!(validate_effective(&snapshot, &effective).is_err());
    }
}
