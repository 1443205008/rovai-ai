//! Explicit local smoke helper: fixed fake keys, native config in an isolated fixture only.
//! Run through scripts/smoke-runtime-custom-api.py. Keys are never accepted as arguments.
//! An explicit official-acceptance marker allows existing isolated native login only.
use anyhow::{Context, Result, ensure};
use rovai_core::{
    agent_profile::AdapterKind,
    runtime_custom_api::{
        self, ApiKeyChange, ConnectionMode, CustomApiConfiguration, FieldEdit, native, native_edit,
    },
};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    ensure!(
        args.len() == 3,
        "usage: custom_api_native_fixture EXECUTABLE FIXTURE_ROOT CONFIG_JSON"
    );
    let [executable, root, config] = args.as_slice() else {
        unreachable!()
    };
    ensure!(
        root.is_absolute() && root.join(".rovai-custom-api-fixture").is_file(),
        "fixture marker missing"
    );
    let mut configuration: CustomApiConfiguration =
        serde_json::from_slice(&std::fs::read(config)?)?;
    if configuration.enabled() {
        configuration.validate(configuration.kind())?;
    }
    let kind = configuration.kind();
    let directory = root.join(if kind == AdapterKind::CodexCli {
        "codex"
    } else {
        "claude"
    });
    let fixture_home = if root.join(".rovai-official-login-acceptance").is_file() {
        root.join("home")
    } else {
        root.clone()
    };
    let mut environment = BTreeMap::from([
        (
            "PATH".into(),
            std::env::var("PATH").context("PATH missing")?,
        ),
        ("HOME".into(), fixture_home.to_string_lossy().into_owned()),
        (
            "USERPROFILE".into(),
            fixture_home.to_string_lossy().into_owned(),
        ),
        (
            if kind == AdapterKind::CodexCli {
                "CODEX_HOME"
            } else {
                "CLAUDE_CONFIG_DIR"
            }
            .into(),
            directory.to_string_lossy().into_owned(),
        ),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
        ("DO_NOT_TRACK".into(), "1".into()),
    ]);
    let shell_credential =
        kind == AdapterKind::ClaudeCodeCli && root.join("shell-credential-fixture").is_file();
    if shell_credential {
        environment.insert(
            "ANTHROPIC_AUTH_TOKEN".into(),
            "rovai-isolated-fake-key".into(),
        );
    }
    if root.join("official-oauth-fixture").is_file() && kind == AdapterKind::ClaudeCodeCli {
        environment.insert(
            "CLAUDE_CODE_OAUTH_TOKEN".into(),
            "fake-official-oauth-token".into(),
        );
    }
    let context = native::NativeContext {
        kind,
        directory,
        artifact_root: root.join("derived"),
        environment,
    };
    let before = native::read(&context, Some(ConnectionMode::CustomApi))?;
    let fields = if kind == AdapterKind::CodexCli {
        vec![vec!["baseUrl"], vec!["codexModels"], vec!["defaultRowId"]]
    } else {
        vec![
            vec!["baseUrl"],
            vec!["claudeModels", "model"],
            vec!["claudeModels", "reasoningModel"],
            vec!["claudeModels", "haikuModel"],
            vec!["claudeModels", "sonnetModel"],
            vec!["claudeModels", "opusModel"],
        ]
    };
    let edits = fields
        .into_iter()
        .map(|path| FieldEdit {
            path: path.into_iter().map(str::to_owned).collect(),
            before: Value::Null,
            after: Value::Null,
            label: String::new(),
        })
        .collect::<Vec<_>>();
    if configuration.enabled() && !root.join("reuse-native-fixture").is_file() {
        let catalog = if kind == AdapterKind::CodexCli {
            Some(
                runtime_custom_api::codex_catalog::generate(
                    Some(executable),
                    &context,
                    &before,
                    &configuration,
                )
                .await?,
            )
        } else {
            None
        };
        native_edit::write(
            &context,
            &before,
            &configuration,
            &edits,
            &if shell_credential {
                ApiKeyChange::Keep
            } else {
                ApiKeyChange::Replace {
                    value: if root.join("rotate-fixture-key").exists() {
                        "rovai-isolated-rotated-key"
                    } else {
                        "rovai-isolated-fake-key"
                    }
                    .into(),
                }
            },
            catalog.as_ref(),
        )?;
    }
    if configuration.mode() == Some(ConnectionMode::OfficialLogin) {
        native_edit::write(
            &context,
            &before,
            &configuration,
            &[FieldEdit {
                path: vec!["mode".into()],
                before: Value::Null,
                after: Value::Null,
                label: String::new(),
            }],
            &ApiKeyChange::Keep,
            None,
        )?;
    }
    let read = native::read(&context, configuration.mode())?;
    // Expose only the parsed catalog path to the isolated smoke owner. No second
    // TOML parser or credential projection is needed in the Python fixture.
    std::fs::write(
        root.join("catalog-path.json"),
        serde_json::to_vec(&read.catalog_path)?,
    )?;
    let snapshot = read.snapshot(&context, true);
    if root.join("reuse-native-fixture").is_file() && kind == AdapterKind::CodexCli {
        ensure!(
            snapshot.configured_model_ids.is_none()
                && snapshot.model_is_configured("rovai-unknown"),
            "inherited default restricted another member model"
        );
    }

    let mut command = Command::new(executable);
    command
        .env_clear()
        .envs(&context.environment)
        .current_dir(root);
    match &snapshot.configuration {
        CustomApiConfiguration::Codex { .. } => {
            command.args(["app-server", "--listen", "stdio://"]);
        }
        CustomApiConfiguration::ClaudeCode { .. } => {
            command.args([
                "--print",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-prompt-tool",
                "stdio",
                "--no-session-persistence",
            ]);
        }
    }
    // Launch with only the saved native connection. The smoke owner observes
    // native protocol results, without production connection/auth preflights.
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = child.stdin.take().context("native stdin missing")?;
    let input =
        tokio::spawn(async move { tokio::io::copy(&mut tokio::io::stdin(), &mut input).await });
    let mut lines = BufReader::new(child.stdout.take().context("native stdout missing")?).lines();
    let mut output = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        output.write_all(line.as_bytes()).await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
    }
    input.abort();
    let status = child.wait().await?;
    ensure!(status.success(), "native fixture failed: {}", status);
    Ok(())
}
