//! Claude Code's PermissionRequest hook is a private approval transport. It
//! does not expose Rovai business tools or replace Claude's native policy.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    action::{CanonicalActionInput, RuntimeActionRequestBinding, RuntimePermissionOption},
    command::canonical_json_digest,
};

pub const CLAUDE_PERMISSION_HOOK_IPC_KIND: &str = "claude_permission_hook";
pub const CLAUDE_PERMISSION_HOOK_IPC_VERSION: u32 = 1;
pub const CLAUDE_PERMISSION_NATIVE_METHOD: &str = "claude/permission_request";

pub struct ClaudePermissionAction {
    pub action_id: String,
    pub native_action_id: String,
    pub input: CanonicalActionInput,
    pub runtime_request: RuntimeActionRequestBinding,
    pub reason: Option<String>,
}

pub fn deny_decision(message: &str) -> Value {
    json!({"behavior": "deny", "message": message})
}

pub fn hook_output(decision: Value) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision,
        }
    })
}

pub fn intercepted_action_request(
    agent_run_id: &str,
    execution_epoch: i64,
    expected_session_id: &str,
    execution_root: &Path,
    request_id: &str,
    native_tool_call_id: &str,
    hook: &Value,
) -> Result<ClaudePermissionAction> {
    if hook.get("hook_event_name").and_then(Value::as_str) != Some("PermissionRequest")
        || hook.get("session_id").and_then(Value::as_str) != Some(expected_session_id)
    {
        bail!("Claude permission hook is outside the active Native Session");
    }
    let tool_name = hook
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .context("Claude permission hook has no tool name")?;
    let tool_input = hook
        .get("tool_input")
        .filter(|input| input.is_object())
        .context("Claude permission hook has no object tool input")?;
    let root = execution_root
        .to_str()
        .context("Claude execution root is not UTF-8")?;
    if !execution_root.is_absolute() {
        bail!("Claude execution root must be absolute");
    }
    let request_digest = canonical_json_digest(&json!({
        "requestId": request_id,
        "hook": hook,
    }))?;
    let cwd = hook
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| Path::new(cwd).is_absolute())
        .unwrap_or(root);
    let input = match tool_name {
        "Bash" => match tool_input.get("command").and_then(Value::as_str) {
            Some(command) if !command.trim().is_empty() => CanonicalActionInput::ShellCommand {
                argv: vec![command.into()],
                cwd: cwd.into(),
                environment_refs: Vec::new(),
                command_transport: None,
            },
            _ => bail!("Claude Bash permission hook has no command"),
        },
        "Write" | "Edit" | "NotebookEdit" => {
            let path = tool_input
                .get("file_path")
                .or_else(|| tool_input.get("notebook_path"))
                .and_then(Value::as_str)
                .context("Claude file permission hook has no path")?;
            let path = Path::new(path);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                Path::new(cwd).join(path)
            };
            CanonicalActionInput::FileWrite {
                path: path.to_string_lossy().into_owned(),
                operation: if tool_name == "Write" {
                    "create"
                } else {
                    "patch"
                }
                .into(),
                content_digest: request_digest.clone(),
            }
        }
        "Read" => CanonicalActionInput::SensitiveRead {
            resource: tool_input
                .get("file_path")
                .and_then(Value::as_str)
                .context("Claude read permission hook has no path")?
                .into(),
        },
        name if name.starts_with("mcp__") => {
            let (server, tool) = name[5..]
                .split_once("__")
                .context("Claude MCP permission hook has no server/tool pair")?;
            CanonicalActionInput::McpTool {
                server: server.into(),
                tool: tool.into(),
                arguments: tool_input.clone(),
            }
        }
        _ => CanonicalActionInput::RuntimePermissionGrant {
            cwd: cwd.into(),
            permissions: json!({"toolName": tool_name, "toolInput": tool_input}),
            request_digest: request_digest.clone(),
        },
    };
    let allow = json!({"behavior": "allow", "updatedInput": tool_input});
    let deny = deny_decision("Rovai 用户拒绝了这次 Claude Code 操作");
    let action_id_digest = canonical_json_digest(&json!({
        "agentRunId": agent_run_id,
        "executionEpoch": execution_epoch,
        "nativeMethod": CLAUDE_PERMISSION_NATIVE_METHOD,
        "requestId": request_id,
    }))?;
    Ok(ClaudePermissionAction {
        action_id: format!("action-{action_id_digest}"),
        native_action_id: request_id.into(),
        input,
        runtime_request: RuntimeActionRequestBinding {
            native_method: CLAUDE_PERMISSION_NATIVE_METHOD.into(),
            native_request_id: Value::String(request_id.into()),
            native_item_id: native_tool_call_id.into(),
            native_thread_id: expected_session_id.into(),
            native_turn_id: format!("claude-code:{agent_run_id}:{execution_epoch}"),
            response_context: hook.clone(),
            options: vec![
                RuntimePermissionOption::from_native(
                    "claude.deny",
                    "deny",
                    "拒绝",
                    "拒绝这一次 Claude Code 操作。",
                    deny,
                    false,
                )?,
                RuntimePermissionOption::from_native(
                    "claude.allow_once",
                    "allow_once",
                    "允许一次",
                    "仅允许这一次 Claude Code 操作，不保存规则。",
                    allow,
                    true,
                )?,
            ],
        },
        reason: tool_input
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(tool_name.into())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_hook_binds_session_and_preserves_exact_tool_input() {
        let hook = json!({
            "hook_event_name": "PermissionRequest",
            "session_id": "session-1",
            "cwd": "/tmp/project",
            "tool_name": "Bash",
            "tool_input": {"command": "rovai send --public-only --body hello", "description": "Send update"}
        });
        let action = intercepted_action_request(
            "run-1",
            3,
            "session-1",
            Path::new("/tmp/project"),
            "request-1",
            "tool-1",
            &hook,
        )
        .unwrap();
        match &action.input {
            CanonicalActionInput::ShellCommand { argv, .. } => assert_eq!(
                argv,
                &vec!["rovai send --public-only --body hello".to_string()]
            ),
            _ => panic!("Claude Bash must keep the exact command for Approval"),
        }
        assert_eq!(
            action.runtime_request.options[1].native_response["updatedInput"],
            hook["tool_input"]
        );
        assert_eq!(
            action.runtime_request.options[0].native_response["behavior"],
            "deny"
        );
        assert!(
            intercepted_action_request(
                "run-1",
                3,
                "session-2",
                Path::new("/tmp/project"),
                "request-1",
                "tool-1",
                &hook,
            )
            .is_err()
        );
    }
}
