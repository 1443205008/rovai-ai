//! Inspect the launched native process's resolved configuration, without a model request.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio::process::Command;
use super::{CustomApiConfiguration, CustomApiSnapshot};

pub fn configure(snapshot: &CustomApiSnapshot, command: &mut Command) -> Result<()> {
    let settings = snapshot.claude_settings()?;
    command.envs(settings["env"].as_object().context("Claude 环境配置无效。")?.iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default())));
    let path = snapshot.write_artifact("claude-settings.json", &serde_json::to_vec(&settings)?)?;
    command.arg("--settings").arg(path);
    Ok(())
}

pub fn validate(snapshot: &CustomApiSnapshot, settings: &Value, status: &Value, explicit: Option<&str>) -> Result<()> {
    let CustomApiConfiguration::ClaudeCode { models, base_url, .. } = &snapshot.configuration else {
        anyhow::bail!("Claude 自定义 API 类型不匹配。");
    };
    let requested = snapshot.claude_settings()?;
    let effective = settings.pointer("/effective/env").and_then(Value::as_object)
        .context("当前 Claude Code 无法报告最终环境配置，不能确认自定义 API 已生效。")?;
    for (name, expected) in requested["env"].as_object().context("Claude 环境配置无效。")? {
        ensure!(effective.get(name) == Some(expected), "Claude Code 原生设置或组织策略覆盖了 {}；自定义 API 未生效。", name);
    }
    let rows = status.get("sections").and_then(Value::as_array).context("Claude Code 未报告实际认证来源。")?
        .iter().filter_map(|section| section.get("rows").and_then(Value::as_array)).flatten().collect::<Vec<_>>();
    let row = |label: &str| rows.iter().find(|row| row["label"].as_str() == Some(label)).and_then(|row| row["value"].as_str());
    ensure!(row("Auth token") == Some("ANTHROPIC_AUTH_TOKEN"), "Claude Code 未使用本次配置的 Bearer 密钥；请检查原生认证或组织策略冲突。");
    ensure!(row("Anthropic base URL") == Some(base_url.as_str()), "Claude Code 的实际接口地址与本次配置不一致。");
    let model = explicit.or_else(|| snapshot.configuration.default_model());
    let expected = match model {
        Some("haiku") => (!models.haiku_model.is_empty()).then_some(models.haiku_model.as_str()),
        Some("sonnet") => (!models.sonnet_model.is_empty()).then_some(models.sonnet_model.as_str()),
        Some("opus") => (!models.opus_model.is_empty()).then_some(models.opus_model.as_str()),
        // Native compound aliases remain runtime-owned.
        Some("default" | "opusplan" | "sonnet[1m]" | "opus[1m]") => None,
        id => id,
    };
    if let Some(expected) = expected {
        ensure!(settings.pointer("/applied/model").and_then(Value::as_str) == Some(expected),
            "Claude Code 的实际模型与本次选择不一致，请检查原生模型映射。");
    }
    Ok(())
}
