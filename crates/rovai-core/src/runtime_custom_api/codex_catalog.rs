//! Preserve native catalogs; new metadata comes from the actual selected entrypoint.
//! Known metadata is read from the selected executable, never another installation/cache.
use super::CustomApiConfiguration;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

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
    let selected_ids = models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    anyhow::ensure!(
        selected_ids.len() == models.len(),
        "Codex 模型 ID 不能重复。"
    );
    let managed_list =
        catalog["rovai_managed_model_list"] == true || model_ids_changed(current, desired);
    let native = catalog["models"].as_array_mut().unwrap();
    let original = native.clone();
    for row in previous
        .iter()
        .filter(|old| !selected_ids.contains(old.id.as_str()))
    {
        if let Some(item) = native.iter_mut().find(|item| item["slug"] == row.id) {
            item["visibility"] = json!("hide");
        }
    }
    for model in models {
        let previous_row = previous.iter().find(|old| old.row_id == model.row_id);
        let old_id = previous_row.map(|row| row.id.as_str()).unwrap_or(&model.id);
        // A hidden/native ID already owns its metadata. Reuse that entry instead
        // of renaming another entry onto it and duplicating the slug.
        let destination = original.iter().position(|entry| entry["slug"] == model.id);
        let source = original.iter().position(|entry| entry["slug"] == old_id);
        let mut item = destination
            .or(source)
            .map(|index| original[index].clone())
            .or_else(|| {
                bundled
                    .and_then(|c| c["models"].as_array())
                    .and_then(|entries| entries.iter().find(|entry| entry["slug"] == model.id))
                    .cloned()
            })
            .unwrap_or_else(|| fallback_model(&model.id));
        item["slug"] = json!(model.id);
        if !inherited_catalog || previous_row.is_none_or(|old| old.id != model.id) {
            // Explicit API selection, not a claim about service-side capabilities.
            item["visibility"] = json!("list");
            item["supported_in_api"] = json!(true);
        }
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
        // Another row may still select old_id (a swap or chained rename). Keep
        // that slot in this case, using the immutable original as metadata input.
        let slot = destination.or(source.filter(|_| !selected_ids.contains(old_id)));
        if let Some(index) = slot {
            native[index] = item;
        } else {
            native.push(item);
        }
    }
    catalog["rovai_managed_model_list"] = json!(managed_list);
    if managed_list {
        catalog["rovai_model_ids"] = json!(models.iter().map(|m| &m.id).collect::<Vec<_>>());
    }
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
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| entry["slug"] == *id || entry["slug"] == model.id)
                })
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
        known["supported_in_api"] = json!(false);
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
        assert_eq!(
            models[0]["supported_in_api"], true,
            "an explicitly added API model is selectable natively"
        );
        assert_eq!(models[1]["slug"], "native-internal");
        assert_eq!(models[1]["visibility"], "none");
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
        expected["rovai_model_ids"][0] = json!("custom-renamed-id");
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
        // Hidden collisions, swaps and chained renames must each produce one
        // entry per ID. An existing target keeps its own complete metadata.
        for ids in [
            ["native-internal", "known-but-unknown-suffix"],
            ["known-but-unknown-suffix", "known"],
            ["native-internal", "known"],
            ["known-but-unknown-suffix", "new-id"],
        ] {
            let mut renamed = config.clone();
            if let CustomApiConfiguration::Codex {
                models,
                default_model,
                ..
            } = &mut renamed
            {
                for (model, id) in models.iter_mut().zip(ids) {
                    model.id = id.into();
                }
                *default_model = ids[1].into();
            }
            let updated = adapt_catalog(original.clone(), &config, &renamed, None, true).unwrap();
            let entries = updated["models"].as_array().unwrap();
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry["slug"].as_str().unwrap())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                entries.len()
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry["visibility"] == "list")
                    .map(|entry| entry["slug"].as_str().unwrap())
                    .collect::<std::collections::BTreeSet<_>>(),
                ids.into_iter().collect()
            );
            for id in ids {
                if let Some(before) = original["models"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["slug"] == id)
                {
                    let mut expected = before.clone();
                    let actual = entries.iter().find(|entry| entry["slug"] == id).unwrap();
                    expected["visibility"] = json!("list");
                    expected["display_name"] = actual["display_name"].clone();
                    assert_eq!(*actual, expected, "existing ID metadata is retained");
                }
            }
        }
        let mut added = config.clone();
        if let CustomApiConfiguration::Codex { models, .. } = &mut added {
            models.push(super::super::CustomApiModel {
                row_id: "three".into(),
                id: "native-internal".into(),
                display_name: "Existing hidden model".into(),
            });
        }
        let updated = adapt_catalog(original.clone(), &config, &added, None, true).unwrap();
        assert_eq!(updated["models"].as_array().unwrap().len(), 3);
        assert_eq!(
            updated["models"][1]["display_name"],
            "Existing hidden model"
        );
    }
}
