//! Explicit native smoke helper. It uses a fixed fake key and an isolated fixture Home.
//! Run only through scripts/smoke-runtime-custom-api.py; never a production launch path.
use anyhow::{Context, Result, ensure};
use rovai_core::runtime_custom_api::{self, CustomApiConfiguration, CustomApiSnapshot};
use std::path::PathBuf;
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args_os().skip(1).map(PathBuf::from).collect::<Vec<_>>();
    ensure!(args.len() == 3, "usage: custom_api_native_fixture EXECUTABLE FIXTURE_ROOT CONFIG_JSON");
    let [executable, root, config] = args.as_slice() else { unreachable!() };
    ensure!(root.join(".rovai-custom-api-fixture").is_file(), "fixture marker missing");
    let mut configuration: CustomApiConfiguration = serde_json::from_slice(&std::fs::read(config)?)?;
    configuration.validate(configuration.kind())?;
    let store = root.join("host-private");
    let credential_version = runtime_custom_api::write_credential(&store, if root.join("rotate-fixture-key").exists() { "rovai-isolated-rotated-key" } else { "rovai-isolated-fake-key" })?;
    let snapshot = CustomApiSnapshot { configuration, revision: 1, credential_version, storage_root: store };
    let mut command = Command::new(executable);
    command.env_clear().env("PATH", std::env::var_os("PATH").context("PATH missing")?)
        .env("HOME", root).env("USERPROFILE", root).env("CODEX_HOME", root.join("codex"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude")).env("KIMI_CODE_HOME", root.join("kimi"))
        .env("GROK_HOME", root.join("grok")).env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DO_NOT_TRACK", "1").current_dir(root);
    match &snapshot.configuration {
        CustomApiConfiguration::Codex { .. } => {
            runtime_custom_api::codex_catalog::configure(&snapshot, &mut command)?;
            command.args(["app-server", "--listen", "stdio://"]);
        }
        CustomApiConfiguration::ClaudeCode { .. } => {
            runtime_custom_api::claude_native::configure(&snapshot, &mut command)?;
            command.args(["--print", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--permission-prompt-tool", "stdio", "--no-session-persistence"]);
        }
        CustomApiConfiguration::KimiCode { .. } => { snapshot.configure_environment(&mut command)?; command.arg("acp"); }
        CustomApiConfiguration::GrokBuild { .. } => {
            runtime_custom_api::grok_native::configure(&snapshot, &mut command, None)?;
            command.args(["--no-auto-update", "agent", "--no-leader", "stdio"]);
        }
    }
    let status = command.status().await?;
    ensure!(status.success(), "native fixture failed: {status}");
    Ok(())
}
