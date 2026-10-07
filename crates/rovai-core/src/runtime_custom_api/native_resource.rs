//! Local structural validation for native model catalog files.
use anyhow::{Context, Result, ensure};
use serde_json::Value;

pub(super) fn validate_catalog(catalog: &Value) -> Result<()> {
    let entries = catalog
        .get("models")
        .and_then(Value::as_array)
        .context("Codex 本地输出不是完整模型目录。")?;
    let mut ids = std::collections::BTreeSet::new();
    for entry in entries {
        let id = entry
            .get("slug")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .context("Codex 原生模型条目缺少 slug。")?;
        ensure!(ids.insert(id), "Codex 原生目录含重复模型 ID。");
        ensure!(
            entry.get("display_name").is_some(),
            "Codex 原生模型条目不完整。"
        );
    }
    ensure!(!entries.is_empty(), "Codex 原生目录为空。");
    Ok(())
}
