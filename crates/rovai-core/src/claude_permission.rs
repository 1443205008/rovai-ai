//! Convert native Claude Code can_use_tool requests into existing Actions
//! and frozen Approval options. Native policy remains Claude's authority.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    action::{CanonicalActionInput, RuntimeActionRequestBinding, RuntimePermissionOption},
    command::canonical_json_digest,
};

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

pub fn intercepted_action_request(
    agent_run_id: &str,
    execution_epoch: i64,
    expected_session_id: &str,
    execution_root: &Path,
    control_request: &Value,
) -> Result<ClaudePermissionAction> {
    if control_request.get("type").and_then(Value::as_str) != Some("control_request")
        || control_request
            .get("session_id")
            .is_some_and(|id| id.as_str() != Some(expected_session_id))
    {
        bail!("Claude permission request is outside the active Native Session");
    }
    let request_id = control_request
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .context("Claude permission request has no request_id")?;
    let request = &control_request["request"];
    if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
        bail!("Claude control request is not a tool permission request");
    }
    let native_tool_call_id = request
        .get("tool_use_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .context("Claude permission request has no reliable tool_use_id")?;
    let tool_name = request
        .get("tool_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .context("Claude permission request has no tool name")?;
    let tool_input = request
        .get("input")
        .filter(|input| input.is_object())
        .context("Claude permission request has no object tool input")?;
    let root = execution_root
        .to_str()
        .context("Claude execution root is not UTF-8")?;
    if !execution_root.is_absolute() {
        bail!("Claude execution root must be absolute");
    }
    let request_digest = canonical_json_digest(&json!({
        "requestId": request_id,
        "request": control_request,
    }))?;
    let cwd = root;
    let input = match tool_name {
        "Bash" => match tool_input.get("command").and_then(Value::as_str) {
            Some(command) if !command.trim().is_empty() => CanonicalActionInput::ShellCommand {
                argv: vec![command.into()],
                cwd: cwd.into(),
                environment_refs: Vec::new(),
                command_transport: None,
            },
            _ => bail!("Claude Bash permission request has no command"),
        },
        "Write" | "Edit" | "NotebookEdit" => {
            let path = tool_input
                .get("file_path")
                .or_else(|| tool_input.get("notebook_path"))
                .and_then(Value::as_str)
                .context("Claude file permission request has no path")?;
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
                .context("Claude read permission request has no path")?
                .into(),
        },
        name if name.starts_with("mcp__") => {
            let (server, tool) = name[5..]
                .split_once("__")
                .context("Claude MCP permission request has no server/tool pair")?;
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
            response_context: control_request.clone(),
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
    fn native_permission_ids_and_exact_input_bind_frozen_approval_options() {
        let request = json!({"type":"control_request", "request_id":"request-1", "session_id":"session-1",
            "request":{"subtype":"can_use_tool", "tool_use_id":"tool-1", "tool_name":"Bash",
                "input":{"command":"rovai send --public-only --body hello", "description":"Send update"}}});
        let convert = |request: &Value| {
            intercepted_action_request("run-1", 3, "session-1", Path::new("/tmp/project"), request)
        };
        let action = convert(&request).unwrap();
        match &action.input {
            CanonicalActionInput::ShellCommand { argv, cwd, .. } => {
                assert_eq!(
                    argv,
                    &vec!["rovai send --public-only --body hello".to_string()]
                );
                assert_eq!(cwd, "/tmp/project");
            }
            _ => panic!("Claude Bash must keep the exact command for Approval"),
        }
        assert_eq!(action.runtime_request.native_request_id, "request-1");
        assert_eq!(action.runtime_request.native_item_id, "tool-1");
        assert_eq!(action.runtime_request.native_thread_id, "session-1");
        assert_eq!(
            action.runtime_request.options[1].native_response["updatedInput"],
            request["request"]["input"]
        );
        assert!(
            action.runtime_request.options[1]
                .native_response
                .get("updatedPermissions")
                .is_none()
        );
        assert_eq!(
            action.runtime_request.options[0].native_response["behavior"],
            "deny"
        );
        for (pointer, invalid) in [
            ("session_id", json!("session-2")),
            ("request_id", json!("")),
        ] {
            let mut value = request.clone();
            value[pointer] = invalid;
            assert!(convert(&value).is_err());
        }
        for (pointer, invalid) in [
            ("tool_use_id", Value::Null),
            ("tool_use_id", json!("")),
            ("input", json!([])),
        ] {
            let mut value = request.clone();
            value["request"][pointer] = invalid;
            assert!(convert(&value).is_err());
        }
        let mut identical = request.clone();
        identical["request_id"] = json!("request-2");
        identical["request"]["tool_use_id"] = json!("tool-2");
        let second = convert(&identical).unwrap();
        assert_ne!(action.action_id, second.action_id);
        assert_eq!(second.runtime_request.native_item_id, "tool-2");
    }
}
