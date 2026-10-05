//! Settings-only account observation through the selected CLI. Never exports a
//! token, tests a model, changes authentication or participates in execution admission.
use super::{
    ConnectionMode, ConnectionObservation, CustomApiConfiguration, NativeCredential,
    native::{self, NativeContext},
};
use crate::runtime_probe_process::{DEFAULT_CLEANUP_TIMEOUT, RuntimeProbeProcess};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, time::timeout};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    Api,
    Official,
    SignedOut,
    Unknown,
}
impl Identity {
    pub fn from_account(value: &Value) -> Self {
        match value.pointer("/account/type").and_then(Value::as_str) {
            Some("apiKey") => Self::Api,
            Some("chatgpt") => Self::Official,
            None if value.get("account") == Some(&Value::Null)
                && value["requiresOpenaiAuth"] == true =>
            {
                Self::SignedOut
            }
            _ => Self::Unknown,
        }
    }
}
type Hints = BTreeMap<PathBuf, (String, Identity)>;
static HINTS: OnceLock<Mutex<Hints>> = OnceLock::new();
fn evidence(context: &NativeContext) -> Result<String> {
    let configuration = native::codex_config(context)?;
    let fallback = if configuration["cli_auth_credentials_store"] == "auto" {
        native::read_bytes(&context.directory.join("auth.json"))
            .ok()
            .flatten()
    } else {
        None
    };
    crate::command::canonical_json_digest(&json!([
        configuration,
        fallback,
        context.env("OPENAI_API_KEY"),
        context.env("CODEX_API_KEY"),
        context.env("CODEX_ACCESS_TOKEN")
    ]))
}
pub fn needs_observation(context: &NativeContext) -> bool {
    context.kind == crate::agent_profile::AdapterKind::CodexCli
        && native::codex_config(context).ok().is_some_and(|doc| {
            matches!(
                doc["cli_auth_credentials_store"].as_str(),
                Some("keyring" | "auto" | "ephemeral")
            )
        })
}
pub fn observed(context: &NativeContext) -> Identity {
    let Ok(evidence) = evidence(context) else {
        return Identity::Unknown;
    };
    HINTS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|hints| {
            hints
                .get(&context.directory)
                .filter(|(before, _)| *before == evidence)
                .map(|(_, identity)| *identity)
        })
        .unwrap_or(Identity::Unknown)
}
pub(crate) fn observed_api(context: &NativeContext) -> bool {
    context.kind == crate::agent_profile::AdapterKind::CodexCli
        && observed(context) == Identity::Api
        && native::codex_config(context)
            .ok()
            .is_some_and(|doc| doc["forced_login_method"] != "chatgpt")
}
pub fn forget(context: &NativeContext) {
    if let Ok(mut hints) = HINTS.get_or_init(Default::default).lock() {
        hints.remove(&context.directory);
    }
}
fn record(context: &NativeContext, before: &str, identity: Identity) {
    if let Ok(mut hints) = HINTS.get_or_init(Default::default).lock() {
        hints.remove(&context.directory);
        if evidence(context).ok().as_deref() == Some(before) {
            if hints.len() >= 64 {
                hints.clear();
            }
            hints.insert(context.directory.clone(), (before.into(), identity));
        }
    }
}
pub fn project(
    context: &NativeContext,
    configuration: &mut CustomApiConfiguration,
    credential: &mut NativeCredential,
    observation: &mut ConnectionObservation,
) {
    if !needs_observation(context) {
        return;
    }
    let identity = observed(context);
    observation.login_status = match identity {
        Identity::Official => "signed_in",
        Identity::Api | Identity::SignedOut => "signed_out",
        Identity::Unknown => "unknown",
    }
    .into();
    if configuration.mode().is_none() {
        let mode = match identity {
            Identity::Api => Some(ConnectionMode::CustomApi),
            Identity::Official | Identity::SignedOut => Some(ConnectionMode::OfficialLogin),
            Identity::Unknown => None,
        };
        configuration.set_mode(mode);
        observation.initial_mode = mode;
        if identity == Identity::Api {
            if let CustomApiConfiguration::Codex { base_url, .. } = configuration {
                if base_url.is_empty() {
                    *base_url = "https://api.openai.com/v1".into();
                }
            }
        }
    }
    credential.status = match identity {
        Identity::Api if configuration.enabled() => "available",
        Identity::Unknown => "unknown",
        _ => "missing",
    }
    .into();
    if configuration.mode() == Some(ConnectionMode::OfficialLogin) && identity == Identity::Api {
        // Older/native policy-conflicted installations must not be labelled as a
        // completed ChatGPT switch merely because a setting was written.
        observation.conflict =
            Some("Codex 仍报告 API 认证，请运行下方原生登录命令完成 ChatGPT 登录。".into());
        configuration.set_mode(None);
        observation.initial_mode = None;
    }
}

/// Called on settings entry/save only. All failures become unknown display state;
/// normal runtime authentication remains the CLI's responsibility.
pub async fn refresh(executable: &Path, context: &NativeContext) {
    forget(context);
    let Ok(before) = evidence(context) else {
        return;
    };
    let identity = read_account(executable, context)
        .await
        .unwrap_or(Identity::Unknown);
    record(context, &before, identity);
}
async fn read_account(executable: &Path, context: &NativeContext) -> Result<Identity> {
    let mut command = tokio::process::Command::new(executable);
    if !context.environment.is_empty() {
        command.env_clear().envs(&context.environment);
    }
    command
        .env("CODEX_HOME", &context.directory)
        .current_dir(&context.directory)
        .args(["app-server", "--listen", "stdio://"]);
    let mut process = RuntimeProbeProcess::spawn(
        &mut command,
        1024 * 1024,
        16 * 1024,
        256 * 1024,
        DEFAULT_CLEANUP_TIMEOUT,
    )?;
    let result = timeout(Duration::from_secs(8), async {
        let (stdin, lines) = process.split_io()?;
        for (id, method, params) in [
            (
                1,
                "initialize",
                json!({"clientInfo":{"name":"rovai_settings","version":env!("CARGO_PKG_VERSION")}}),
            ),
            (2, "account/read", json!({"refreshToken":false})),
        ] {
            let request = serde_json::to_vec(&json!({"id":id,"method":method,"params":params}))?;
            stdin.write_all(&request).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
            loop {
                let line = lines
                    .next_line()
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("native account stream closed"))?;
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if message["id"] != id {
                    continue;
                }
                ensure!(
                    message.get("error").is_none(),
                    "native account read unavailable"
                );
                if id == 2 {
                    return Ok(Identity::from_account(&message["result"]));
                }
                stdin
                    .write_all(b"{\"method\":\"initialized\",\"params\":{}}\n")
                    .await?;
                stdin.flush().await?;
                break;
            }
        }
        Ok(Identity::Unknown)
    })
    .await;
    // Never include native stderr, account details or a raw RPC error in settings/logs.
    let _ = process.finish().await;
    result.map_err(|_| anyhow::anyhow!("native account read timed out"))?
}
