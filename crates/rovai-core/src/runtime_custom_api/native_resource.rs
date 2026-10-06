//! Read local metadata through the selected entrypoint, including npm and shell launchers.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

pub(super) async fn bundled_catalog(
    executable: &Path,
    context: &super::native::NativeContext,
) -> Result<Value> {
    let mut command = tokio::process::Command::new(executable);
    if !context.environment.is_empty() {
        command.env_clear().envs(&context.environment);
    }
    command.env("CODEX_HOME", &context.directory);
    // --bundled returns the executable's own complete catalog without configuration,
    // authentication or a model-list refresh. Never retry without this flag.
    command.args(["debug", "models", "--bundled"]);
    let mut limits =
        crate::runtime_probe_process::ProbeCommandLimits::new(std::time::Duration::from_secs(10));
    limits.stdout_bytes = 8 * 1024 * 1024;
    let output = crate::runtime_probe_process::run_bounded_command(&mut command, limits).await;
    if let Ok(output) = output {
        if output.status.success() && !output.stdout.truncated {
            if let Ok(catalog) = serde_json::from_slice::<Value>(&output.stdout.bytes) {
                if validate_catalog(&catalog).is_ok() {
                    return Ok(catalog);
                }
            }
        }
    }
    // Older native binaries may not expose the local debug command. Parse their
    // own embedded resource without a version/hash allowlist; never read another install.
    embedded_json(executable).map_err(|_| anyhow::anyhow!(
        "当前 Codex 入口无法提供完整本地模型目录（debug models --bundled）。已有目录可继续编辑；新增目录或恢复官方目录时，请使用能输出完整目录的 Codex 入口。原配置未被修改。"
    ))
}
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
fn embedded_json(executable: &Path) -> Result<Value> {
    let marker = b"{\n  \"models\": [";
    let mut file = File::open(executable).context("无法读取所选运行时程序的原生目录。")?;
    let mut scan = Vec::new();
    let mut block = [0_u8; 64 * 1024];
    let mut found = false;
    let mut total = 0_usize;
    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        total += read;
        ensure!(
            total <= 512 * 1024 * 1024,
            "所选运行时程序超出目录读取范围。"
        );
        scan.extend_from_slice(&block[..read]);
        if !found {
            if let Some(position) = scan.windows(marker.len()).position(|bytes| bytes == marker) {
                scan.drain(..position);
                found = true;
            } else {
                let retain = scan.len().saturating_sub(marker.len());
                scan.drain(..retain);
                continue;
            }
        }
        ensure!(scan.len() <= 2 * 1024 * 1024, "原生目录超出受支持大小。");
        let mut stream = serde_json::Deserializer::from_slice(&scan).into_iter::<Value>();
        match stream.next() {
            Some(Ok(catalog)) => {
                validate_catalog(&catalog)?;
                return Ok(catalog);
            }
            Some(Err(error)) if error.is_eof() => continue,
            _ => anyhow::bail!("所选运行时程序的原生目录无法解析。"),
        }
    }
    anyhow::bail!("所选程序没有可读取的内嵌模型目录。")
}

#[cfg(all(test, unix, feature = "extended-tests"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // Owns entrypoint execution and bounded output parsing; the pure catalog
    // patch test cannot detect accidentally scanning a wrapper or dropping --bundled.
    #[tokio::test]
    async fn selected_wrapper_reads_local_catalog_without_resource_fingerprints() {
        let root =
            std::env::temp_dir().join(format!("rovai-catalog-entrypoint-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let wrapper = root.join("selected-wrapper");
        std::fs::write(&wrapper, "#!/bin/sh\n[ \"$*\" = 'debug models --bundled' ] || exit 3\ncat \"$CODEX_HOME/catalog.json\"\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let context = crate::runtime_custom_api::native::NativeContext {
            kind: crate::agent_profile::AdapterKind::CodexCli,
            directory: root.clone(),
            artifact_root: root.join("artifacts"),
            launcher: None,
            codex_source: None,
            environment: std::collections::BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), root.to_string_lossy().into_owned()),
            ]),
        };
        // A GUI PATH may omit a shim dependency. Reuse the actual launcher's
        // resolved environment instead of the host process's original PATH.
        let mut gui_context = context.clone();
        gui_context
            .environment
            .insert("PATH".into(), "/missing-gui-path".into());
        let mut launcher = tokio::process::Command::new(&wrapper);
        launcher.env("PATH", "/usr/bin:/bin");
        launcher.env_remove("UNUSED_CREDENTIAL");
        let context = gui_context.for_command(&launcher);
        assert_eq!(context.env("PATH").as_deref(), Some("/usr/bin:/bin"));
        use crate::runtime_custom_api::{
            ApiKeyChange, ConnectionMode, CustomApiConfiguration, CustomApiModel, FieldEdit,
            native, native_edit,
        };
        let desired = CustomApiConfiguration::Codex {
            mode: Some(ConnectionMode::CustomApi),
            base_url: "https://fixture.invalid".into(),
            models: vec![CustomApiModel {
                row_id: "example".into(),
                id: "example".into(),
                display_name: "Example".into(),
            }],
            default_model: "example".into(),
            default_row_id: Some("example".into()),
        };
        let mut paths = Vec::new();
        for resource in ["different-resource-a", "different-resource-b"] {
            let catalog = serde_json::json!({"models":[{"slug":"example","display_name":"Example","context_window":1234,"future_field":resource}]});
            std::fs::write(
                root.join("catalog.json"),
                serde_json::to_vec(&catalog).unwrap(),
            )
            .unwrap();
            assert_eq!(bundled_catalog(&wrapper, &context).await.unwrap(), catalog);
            let current = native::read(&context, None).unwrap();
            native_edit::write(
                &context,
                &current,
                &desired,
                &[FieldEdit {
                    path: vec!["codexModels".into()],
                    before: Value::Null,
                    after: Value::Null,
                    label: String::new(),
                }],
                &ApiKeyChange::Keep,
                Some(&catalog),
            )
            .unwrap();
            let path = native::read(&context, None).unwrap().catalog_path.unwrap();
            paths.push(path);
        }
        assert_ne!(
            paths[0], paths[1],
            "changed runtime metadata creates a new file without blocking or overwriting the old one"
        );
        assert_eq!(
            crate::runtime_custom_api::native::read_json(&paths[0]).unwrap()["models"][0]["future_field"],
            "different-resource-a"
        );
        std::fs::write(root.join("catalog.json"), b"not-json secret-from-wrapper").unwrap();
        let error = bundled_catalog(&wrapper, &context)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret-from-wrapper") && !error.contains("关闭自定义 API"));
        assert!(error.contains("完整本地模型目录"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
