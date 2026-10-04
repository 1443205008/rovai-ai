use super::*;
use crate::runtime_startup::{self, RuntimeEnvironmentVariable, RuntimeStartupConfiguration};
pub(super) fn configuration(kind: AdapterKind) -> CustomApiConfiguration {
    let base_url = "https://relay.example/prefix".into();
    match kind {
        AdapterKind::ClaudeCodeCli => CustomApiConfiguration::ClaudeCode {
            mode: Some(ConnectionMode::CustomApi),
            base_url,
            models: ClaudeApiModels {
                model: "main".into(),
                reasoning_model: "think".into(),
                haiku_model: "small".into(),
                sonnet_model: "medium".into(),
                opus_model: "large".into(),
            },
        },
        AdapterKind::CodexCli => CustomApiConfiguration::Codex {
            mode: Some(ConnectionMode::CustomApi),
            base_url,
            models: vec![
                CustomApiModel {
                    row_id: "one".into(),
                    id: "model-a".into(),
                    display_name: "Development".into(),
                },
                CustomApiModel {
                    row_id: "two".into(),
                    id: "model-b".into(),
                    display_name: String::new(),
                },
            ],
            default_model: "model-a".into(),
            default_row_id: Some("one".into()),
        },
        _ => unreachable!(),
    }
}
// This parser owner retains the URL/key/closed-schema boundaries from the superseded secret-store draft.
#[test]
fn configuration_rejects_ambiguous_connections_and_preserves_optional_models() {
    use claude_native::Identity;
    for (account, expected) in [
        (
            json!({"subscriptionType":"max","email":"private@example.invalid"}),
            Identity::Official,
        ),
        (json!({"subscriptionType":null}), Identity::Official),
        (
            json!({"tokenSource":"CLAUDE_CODE_OAUTH_TOKEN"}),
            Identity::Official,
        ),
        (
            json!({"tokenSource":"ANTHROPIC_AUTH_TOKEN","subscriptionType":"max"}),
            Identity::Api("ANTHROPIC_AUTH_TOKEN".into()),
        ),
        (
            json!({"tokenSource":"none","apiKeySource":"ANTHROPIC_API_KEY"}),
            Identity::Api("ANTHROPIC_API_KEY".into()),
        ),
        (
            json!({"tokenSource":"none","apiKeySource":"/login managed key"}),
            Identity::Api("/login managed key".into()),
        ),
        (
            json!({"tokenSource":"apiKeyHelper"}),
            Identity::Api("apiKeyHelper".into()),
        ),
        (json!({"tokenSource":"none"}), Identity::SignedOut),
        (
            json!({"email":"private@example.invalid"}),
            Identity::Unknown,
        ),
        (json!({"apiProvider":"bedrock"}), Identity::ThirdParty),
    ] {
        let response = json!({"account":account,"sections":[{"rows":[{"label":"Auth token","value":"misleading display text"}]}]});
        assert_eq!(Identity::from_initialize(&response), expected);
    }
    for method in ["claude.ai", "oauth_token"] {
        assert_eq!(
            Identity::from_auth_status(
                &json!({"loggedIn":true,"authMethod":method,"apiProvider":"firstParty"})
            ),
            Identity::Official
        );
    }
    assert_eq!(
        native::login_command("codex", None, None, false),
        "codex login"
    );
    assert_eq!(native::login_command("claude", None, None, false), "claude");
    assert_eq!(
        native::login_command(
            "codex",
            Some("/tools/my codex"),
            Some(("CODEX_HOME", "/my config")),
            false
        ),
        "CODEX_HOME='/my config' '/tools/my codex' login"
    );
    assert_eq!(
        native::login_command(
            "claude",
            Some("C:\\tools\\claude.exe"),
            Some(("CLAUDE_CONFIG_DIR", "C:\\config's")),
            true
        ),
        "$env:CLAUDE_CONFIG_DIR='C:\\config''s'; & 'C:\\tools\\claude.exe'"
    );
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli] {
        let mut config = configuration(kind);
        config.validate(kind).unwrap();
        assert_eq!(config.base_url(), "https://relay.example/prefix");
        assert!(config.validate(AdapterKind::Pi).is_err());
    }
    for url in [
        "https://name:password@relay.example/prefix",
        "ftp://relay.example",
        "https://relay.example/#fragment",
        "",
    ] {
        let mut config = CustomApiConfiguration::ClaudeCode {
            mode: Some(ConnectionMode::CustomApi),
            base_url: url.into(),
            models: ClaudeApiModels::default(),
        };
        assert!(config.validate(AdapterKind::ClaudeCodeCli).is_err());
    }
    let mut claude = CustomApiConfiguration::ClaudeCode {
        mode: Some(ConnectionMode::CustomApi),
        base_url: "http://127.0.0.1/prefix".into(),
        models: ClaudeApiModels::default(),
    };
    claude.validate(AdapterKind::ClaudeCodeCli).unwrap();
    let mut codex = configuration(AdapterKind::CodexCli);
    if let CustomApiConfiguration::Codex { models, .. } = &mut codex {
        models.remove(0);
    }
    assert!(codex.validate(AdapterKind::CodexCli).is_err());
    if let CustomApiConfiguration::Codex { default_row_id, .. } = &mut codex {
        *default_row_id = Some("two".into());
    }
    codex.validate(AdapterKind::CodexCli).unwrap();
    if let CustomApiConfiguration::Codex { models, .. } = &mut codex {
        models.push(models[0].clone());
    }
    assert!(codex.validate(AdapterKind::CodexCli).is_err());
    for value in ["", "********", "bad\nkey"] {
        assert!(
            ApiKeyChange::Replace {
                value: value.into()
            }
            .validate()
            .is_err()
        );
    }
    assert!(serde_json::from_value::<CustomApiConfiguration>(json!({"kind":"grok-build","enabled":false,"baseUrl":"","model":"","apiKey":"never-store-me"})).is_err());
    assert!(
        serde_json::from_value::<RuntimeStartupConfiguration>(
            json!({"programPath":null,"environment":[],"customApiSnapshot":{}})
        )
        .is_err()
    );
}

// Native storage + SQLite publication + field-level merge cannot be proved by a serializer-only test.
#[cfg(feature = "extended-tests")]
#[test]
fn native_editor_reads_without_writing_merges_fields_and_never_copies_credentials() {
    let mut db = crate::test_support::seeded_runtime_database_owned();
    let kind = AdapterKind::ClaudeCodeCli;
    let context =
        native::NativeContext::resolve(kind, &RuntimeStartupConfiguration::default(), db.path())
            .unwrap();
    let path = context.path();
    private_storage::atomic_write_private_bytes(&path, br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"isolated-old-key","ANTHROPIC_BASE_URL":"https://relay.example/prefix","ANTHROPIC_MODEL":"before"},"permissions":{"defaultMode":"default"},"unknown":{"preserve":true}}"#).unwrap();
    let before = std::fs::read(&path).unwrap();
    let saved = runtime_startup::load(&db, kind).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "opening the editor is read-only"
    );
    assert_eq!(saved.credential.as_ref().unwrap().status, "available");
    assert_eq!(saved.revision, 0, "native use requires no initial save");
    let frozen = saved.configuration.custom_api_snapshot.clone().unwrap();
    assert_eq!(frozen.key().unwrap().as_deref(), Some("isolated-old-key"));
    let public = serde_json::to_string(&runtime_startup::public(saved.clone())).unwrap();
    assert!(!public.contains("isolated-old-key"));
    let edit = FieldEdit {
        path: vec!["claudeModels".into(), "model".into()],
        before: json!("before"),
        after: json!("after"),
        label: "主模型".into(),
    };
    let mut external = native::read_json(&path).unwrap();
    external["unknown"]["external"] = json!(12);
    private_storage::atomic_write_private_bytes(&path, &serde_json::to_vec(&external).unwrap())
        .unwrap();
    let prepared =
        runtime_startup::prepare_save(&db, kind, vec![edit.clone()], &ApiKeyChange::Keep).unwrap();
    assert!(prepared.conflicts.is_empty());
    runtime_startup::commit_save(&mut db, kind, prepared, 1, ApiKeyChange::Keep, None).unwrap();
    let native = native::read_json(&path).unwrap();
    assert_eq!(native["unknown"]["external"], 12);
    assert_eq!(native["env"]["ANTHROPIC_MODEL"], "after");
    assert_eq!(native["env"]["ANTHROPIC_AUTH_TOKEN"], "isolated-old-key");
    let mut stale = edit.clone();
    stale.after = json!("mine");
    let conflict =
        runtime_startup::prepare_save(&db, kind, vec![stale], &ApiKeyChange::Keep).unwrap();
    assert_eq!(conflict.conflicts.len(), 1);
    assert_eq!(conflict.conflicts[0].current, "after");
    let saved = runtime_startup::load(&db, kind).unwrap();
    let change = ApiKeyChange::Replace {
        value: "isolated-new-key".into(),
    };
    let replace = FieldEdit {
        path: vec!["credentialVersion".into()],
        before: json!(saved.credential.as_ref().unwrap().version),
        after: json!("replace"),
        label: "API Key".into(),
    };
    let prepared = runtime_startup::prepare_save(&db, kind, vec![replace], &change).unwrap();
    let saved = runtime_startup::commit_save(&mut db, kind, prepared, 2, change, None).unwrap();
    assert!(
        frozen.key().is_err(),
        "retained references must not quietly read a newer key"
    );
    assert!(
        frozen.assert_current().is_err(),
        "old executions cannot be resumed against a new connection"
    );
    let stored: String = db
        .connection()
        .query_row(
            "SELECT configuration_json FROM runtime_startup_setting",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        !stored.contains("isolated-new-key")
            && !stored.contains("isolated-old-key")
            && !stored.contains("relay.example")
    );
    let snapshot = saved.configuration.custom_api_snapshot.unwrap();
    let mut message =
        json!({"echo":"isolated-new-key", "isolated-new-key":["prefix isolated-new-key suffix"]});
    snapshot.redactor().unwrap().value(&mut message);
    assert!(!message.to_string().contains("isolated-new-key"));
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("isolated-new-key")
    );
    assert!(
        !snapshot
            .claude_settings()
            .unwrap()
            .to_string()
            .contains("isolated-new-key")
    );
    let artifact = snapshot.write_artifact("immutable.json", b"old").unwrap();
    assert!(
        snapshot
            .write_artifact("immutable.json", b"changed")
            .is_err()
    );
    assert!(snapshot.write_artifact("../escape", b"bad").is_err());
    assert_eq!(std::fs::read(artifact).unwrap(), b"old");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mut preview = RuntimeStartupConfiguration {
        custom_api: Some(configuration(kind)),
        ..Default::default()
    };
    preview.environment.push(RuntimeEnvironmentVariable {
        name: "CLAUDE_CONFIG_DIR".into(),
        value: context.directory.to_string_lossy().into_owned(),
    });
    let bytes = std::fs::read(&path).unwrap();
    let (preview, _) = runtime_startup::resolve_draft(
        &db,
        kind,
        preview,
        ApiKeyChange::Replace {
            value: "preview-only-key".into(),
        },
    )
    .unwrap();
    assert_eq!(
        preview
            .custom_api_snapshot
            .unwrap()
            .key()
            .unwrap()
            .as_deref(),
        Some("preview-only-key")
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let codex_directory = context.directory.join("codex-auto-preview");
    let codex_path = codex_directory.join("config.toml");
    let codex_bytes =
        b"cli_auth_credentials_store='auto'\nopenai_base_url='https://native.example'\n";
    private_storage::atomic_write_private_bytes(&codex_path, codex_bytes).unwrap();
    let draft = RuntimeStartupConfiguration {
        custom_api: Some(configuration(AdapterKind::CodexCli)),
        environment: vec![RuntimeEnvironmentVariable {
            name: "CODEX_HOME".into(),
            value: codex_directory.to_string_lossy().into_owned(),
        }],
        ..Default::default()
    };
    let (draft, _) = runtime_startup::resolve_draft(
        &db,
        AdapterKind::CodexCli,
        draft,
        ApiKeyChange::Replace {
            value: "draft-auto-replacement".into(),
        },
    )
    .unwrap();
    let draft = draft.custom_api_snapshot.unwrap();
    assert_eq!(
        draft.key().unwrap().as_deref(),
        Some("draft-auto-replacement")
    );
    assert!(matches!(
        draft.credential_source,
        native::CredentialSource::Environment { .. }
    ));
    assert_ne!(codex_catalog::execution_provider(&draft).unwrap(), "openai");
    assert_eq!(std::fs::read(codex_path).unwrap(), codex_bytes);
    let current = runtime_startup::load(&db, kind).unwrap();
    let change = ApiKeyChange::Replace {
        value: "rollback-only-key".into(),
    };
    let edit = FieldEdit {
        path: vec!["credentialVersion".into()],
        before: json!(current.credential.unwrap().version),
        after: json!("replace"),
        label: "API Key".into(),
    };
    let prepared = runtime_startup::prepare_save(&db, kind, vec![edit], &change).unwrap();
    db.connection().execute_batch("CREATE TRIGGER reject_startup_update BEFORE UPDATE ON runtime_startup_setting BEGIN SELECT RAISE(FAIL, 'isolated storage failure'); END;").unwrap();
    assert!(runtime_startup::commit_save(&mut db, kind, prepared, 3, change, None).is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "a failed state commit restores the native connection"
    );
    assert_eq!(
        runtime_startup::load(&db, kind).unwrap().revision,
        current.revision
    );
}

// File-based native precedence, comment preservation and read-only credential replacement owner.
#[tokio::test]
async fn native_sources_keep_environment_references_and_replace_only_the_selected_connection() {
    let root = std::env::temp_dir().join(format!("rovai-native-config-{}", uuid::Uuid::new_v4()));
    let context = native::NativeContext {
        kind: AdapterKind::CodexCli,
        directory: root.clone(),
        artifact_root: root.join("artifacts"),
        environment: BTreeMap::from([("RELAY_KEY".into(), "isolated-environment-key".into())]),
    };
    let path = context.path();
    private_storage::atomic_write_private_bytes(&path, b"# keep this comment\nmodel_provider = 'relay'\nmodel = 'custom-id'\n[model_providers.relay]\nname = 'Relay'\nbase_url = 'https://relay.example/prefix'\nwire_api = 'responses'\nenv_key = 'RELAY_KEY'\n[mcp_servers.untouched]\ncommand = 'fixture'\n").unwrap();
    let auth = root.join("auth.json");
    private_storage::atomic_write_private_bytes(
        &auth,
        br#"{"tokens":{"access_token":"fake-official-token"},"unknown":true}"#,
    )
    .unwrap();
    let read = native::read(&context, None).unwrap();
    assert!(read.configured_model_ids.is_none());
    assert!(
        read.snapshot(&context, false)
            .model_is_configured("previously-selected-other-model"),
        "native default is not an allowlist"
    );

    assert!(matches!(
        read.source,
        native::CredentialSource::Environment { .. }
    ));
    assert!(
        !read.credential.source_writable
            && read.credential.can_replace
            && !read.credential.can_clear
    );
    // Unused provider/auth-file/UI changes still participate in save CAS, but do
    // not fence the active connection. Active transport or credential changes do.
    let frozen = read.snapshot(&context, false);
    let original = std::fs::read(&path).unwrap();
    let mut unrelated = native::read_toml(&path).unwrap();
    unrelated["model_providers"]["unused"]["base_url"] = toml_edit::value("https://unused.invalid");
    unrelated["model_providers"]["relay"]["name"] = toml_edit::value("New label");
    unrelated["mcp_servers"]["untouched"]["command"] = toml_edit::value("other-fixture");
    unrelated["tui"]["notifications"] = toml_edit::value(false);
    std::fs::write(&path, unrelated.to_string()).unwrap();
    private_storage::atomic_write_private_bytes(
        &auth,
        br#"{"tokens":{"access_token":"different-unused-token"},"unknown":false}"#,
    )
    .unwrap();
    let refreshed = native::read(&context, None).unwrap();
    assert_ne!(read.revision, refreshed.revision);
    assert_eq!(
        frozen.identity().unwrap(),
        refreshed.snapshot(&context, false).identity().unwrap()
    );
    frozen.assert_current().unwrap();
    unrelated["model_providers"]["relay"]["stream_idle_timeout_ms"] = toml_edit::value(1234);
    std::fs::write(&path, unrelated.to_string()).unwrap();
    assert!(frozen.assert_current().is_err());
    std::fs::write(&path, original).unwrap();
    let read = native::read(&context, None).unwrap();
    let mut desired = read.configuration.clone();
    if let CustomApiConfiguration::Codex { base_url, .. } = &mut desired {
        *base_url = "https://other.example/prefix".into();
    }
    let edit = FieldEdit {
        path: vec!["baseUrl".into()],
        before: json!(read.configuration.base_url()),
        after: json!(desired.base_url()),
        label: "接口地址".into(),
    };
    native_edit::write(
        &context,
        &read,
        &desired,
        &[edit],
        &ApiKeyChange::Keep,
        None,
    )
    .unwrap();
    assert_eq!(
        native::read(&context, None).unwrap().credential.status,
        "available",
        "static keys have no inferred URL binding"
    );
    let read = native::read(&context, None).unwrap();
    let auth_before = std::fs::read(&auth).unwrap();
    native_edit::write(
        &context,
        &read,
        &read.configuration,
        &[],
        &ApiKeyChange::Replace {
            value: "replacement-key".into(),
        },
        None,
    )
    .unwrap();
    let native = std::fs::read_to_string(&path).unwrap();
    assert!(native.contains("# keep this comment") && native.contains("[mcp_servers.untouched]"));
    assert!(!native.contains("isolated-environment-key") && !native.contains("env_key"));
    assert_eq!(
        std::fs::read(&auth).unwrap(),
        auth_before,
        "API edits must not rewrite official tokens"
    );
    let read = native::read(&context, None).unwrap();
    let snapshot = read.snapshot(&context, false);
    let mut command = tokio::process::Command::new("not-spawned");
    codex_catalog::configure(&snapshot, &mut command)
        .await
        .unwrap();
    assert!(
        !format!("{:?}", command.as_std().get_args().collect::<Vec<_>>())
            .contains("replacement-key")
    );
    assert!(
        command
            .as_std()
            .get_envs()
            .any(|(name, value)| name == "ROVAI_CUSTOM_API_KEY"
                && value == Some(std::ffi::OsStr::new("replacement-key")))
    );
    let before = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"bad = \"replacement-key\n").unwrap();
    let error = native::read(&context, None).err().unwrap().to_string();
    assert!(!error.contains("replacement-key"));
    std::fs::write(&path, before).unwrap();
    let mut doc = native::read_toml(&path).unwrap();
    doc["model_providers"]["relay"]["http_headers"]["x-routing-tag"] =
        toml_edit::value("private-routing-tag");
    std::fs::write(&path, doc.to_string()).unwrap();
    let read = native::read(&context, None).unwrap();
    let mut command = tokio::process::Command::new("not-spawned");
    codex_catalog::configure(&read.snapshot(&context, false), &mut command)
        .await
        .unwrap();
    assert!(
        !format!("{:?}", command.as_std().get_args().collect::<Vec<_>>())
            .contains("private-routing-tag")
    );
    assert!(
        command
            .as_std()
            .get_envs()
            .any(|(_, value)| value == Some(std::ffi::OsStr::new("private-routing-tag")))
    );
    doc["model_providers"]["relay"]["http_headers"]["Proxy-Authorization"] =
        toml_edit::value("Basic independent-proxy-secret");
    doc["model_providers"]["relay"]["query_params"]["api-version"] =
        toml_edit::value("fixture-api-version");
    doc["model_providers"]["relay"]["supports_websockets"] = toml_edit::value(true);
    doc["model_providers"]["relay"]["request_max_retries"] = toml_edit::value(2);
    doc["model_providers"]["relay"]["stream_max_retries"] = toml_edit::value(3);
    doc["model_providers"]["relay"]["stream_idle_timeout_ms"] = toml_edit::value(5432);
    doc["model_providers"]["relay"]["websocket_connect_timeout_ms"] = toml_edit::value(2345);
    std::fs::write(&path, doc.to_string()).unwrap();
    let snapshot = native::read(&context, None)
        .unwrap()
        .snapshot(&context, false);
    let mut command = tokio::process::Command::new("not-spawned");
    codex_catalog::configure(&snapshot, &mut command)
        .await
        .unwrap();
    assert_eq!(
        codex_catalog::execution_provider(&snapshot).unwrap(),
        "relay"
    );
    let arguments = command
        .as_std()
        .get_args()
        .map(|v| v.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(arguments.iter().any(|v| v == "model_provider=\"relay\""));
    assert!(
        !arguments
            .iter()
            .any(|v| v.starts_with("model_providers.\"relay\"="))
    );
    for secret in [
        "replacement-key",
        "independent-proxy-secret",
        "fixture-api-version",
    ] {
        assert!(
            !arguments.iter().any(|v| v.contains(secret)),
            "native values must stay out of argv"
        );
    }
    doc["model_providers"]["relay"]["http_headers"]["Authorization"] =
        toml_edit::value("Bearer unrelated-secret");
    std::fs::write(&path, doc.to_string()).unwrap();
    let read = native::read(&context, None).unwrap();
    let error = codex_catalog::configure(
        &read.snapshot(&context, false),
        &mut tokio::process::Command::new("not-spawned"),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("认证请求头") && !error.contains("unrelated-secret"));

    // An unrelated login file must never supply credentials to a provider which
    // does not opt into native OpenAI authentication.
    std::fs::write(&path, "model_provider='relay'\nmodel='custom-id'\n[model_providers.relay]\nbase_url='https://relay.example'\nwire_api='responses'\nrequires_openai_auth=false\n").unwrap();
    private_storage::atomic_write_private_bytes(&auth, br#"{"OPENAI_API_KEY":"inactive-file-key","tokens":{"access_token":"preserved-token"},"unknown":true}"#).unwrap();
    assert!(matches!(
        native::read(&context, None).unwrap().source,
        native::CredentialSource::Missing
    ));

    std::fs::write(&path, "model='custom-id'\nopenai_base_url='https://relay.example'\ncli_auth_credentials_store='keyring'\n").unwrap();
    let read = native::read(&context, None).unwrap();
    assert!(matches!(
        read.source,
        native::CredentialSource::NativeManaged { .. }
    ));
    assert!(read.snapshot(&context, false).key().unwrap().is_none());
    codex_catalog::validate_effective(
        &read.snapshot(&context, false),
        &json!({"config":{"model_provider":"openai","openai_base_url":"https://relay.example"}}),
    )
    .unwrap();
    let mut changed = read.configuration.clone();
    if let CustomApiConfiguration::Codex { base_url, .. } = &mut changed {
        *base_url = "https://new-api.example".into();
    }
    let before = std::fs::read(&path).unwrap();
    assert!(
        native_edit::write(
            &context,
            &read,
            &changed,
            &[FieldEdit {
                path: vec!["baseUrl".into()],
                before: json!("https://relay.example"),
                after: json!("https://new-api.example"),
                label: "接口地址".into()
            }],
            &ApiKeyChange::Keep,
            None
        )
        .unwrap_err()
        .to_string()
        .contains("Codex 原生系统管理")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);

    std::fs::write(
        &path,
        "cli_auth_credentials_store='auto'\nmodel='custom-id'\n",
    )
    .unwrap();
    let auto = native::read(&context, Some(ConnectionMode::OfficialLogin)).unwrap();
    assert!(matches!(
        auto.source,
        native::CredentialSource::NativeManaged { .. }
    ));
    assert_eq!(auto.observation.login_status, "unknown");
    assert!(
        auto.snapshot(&context, true).key().unwrap().is_none(),
        "do not promote fallback file over a keyring credential"
    );
    let fallback = native::codex_auth_file(&context, "auto").unwrap();
    assert_eq!(fallback["OPENAI_API_KEY"], "inactive-file-key");
    assert!(
        native::codex_auth_file(&context, "keyring")
            .unwrap()
            .get("OPENAI_API_KEY")
            .is_none()
    );
    let mut command = tokio::process::Command::new("not-spawned");
    codex_catalog::configure(&auto.snapshot(&context, true), &mut command)
        .await
        .unwrap();
    assert!(
        codex_catalog::validate_account_selection(
            &auto.snapshot(&context, true),
            &json!({"account":{"type":"apiKey"}})
        )
        .is_err()
    );
    codex_catalog::validate_account_selection(
        &auto.snapshot(&context, true),
        &json!({"account":{"type":"chatgpt"}}),
    )
    .unwrap();
    let auto_api = native::read(&context, Some(ConnectionMode::CustomApi)).unwrap();
    for account in [
        json!({"account":null}),
        json!({}),
        json!({"account":{"type":"future-native-identity"}}),
    ] {
        codex_catalog::validate_account_selection(&auto.snapshot(&context, true), &account)
            .unwrap();
        codex_catalog::validate_account_selection(&auto_api.snapshot(&context, true), &account)
            .unwrap();
    }
    let frozen_api = auto_api.snapshot(&context, true);
    let frozen_official = auto.snapshot(&context, true);
    let mut external = fallback.clone();
    external["last_refresh"] = json!("unrelated-native-bookkeeping");
    external["tokens"]["account_id"] = json!("unused-oauth-account");
    std::fs::write(&auth, serde_json::to_vec(&external).unwrap()).unwrap();
    frozen_api.assert_current().unwrap();
    assert!(frozen_api.key().unwrap().is_none());
    external["OPENAI_API_KEY"] = json!("rotated-fallback-key");
    std::fs::write(&auth, serde_json::to_vec(&external).unwrap()).unwrap();
    assert!(frozen_api.assert_current().is_err());
    assert!(frozen_api.key().is_err());
    let current_api = native::read(&context, Some(ConnectionMode::CustomApi)).unwrap();
    assert_ne!(
        current_api.snapshot(&context, true).identity().unwrap(),
        frozen_api.identity().unwrap()
    );
    assert!(
        current_api
            .snapshot(&context, true)
            .key()
            .unwrap()
            .is_none()
    );
    frozen_official.assert_current().unwrap();
    frozen_official.redactor().unwrap();
    std::fs::write(&auth, serde_json::to_vec(&fallback).unwrap()).unwrap();
    // A policy conflict must also protect auto mode's real file fallback from native logout.
    std::fs::write(
        &path,
        "cli_auth_credentials_store='auto'\nforced_login_method='chatgpt'\n",
    )
    .unwrap();
    let auto = native::read(&context, Some(ConnectionMode::OfficialLogin)).unwrap();
    assert!(
        codex_catalog::configure(
            &auto.snapshot(&context, true),
            &mut tokio::process::Command::new("not-spawned")
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("auto")
    );
    assert_eq!(native::read_json(&auth).unwrap(), fallback);
    // Explicit clear affects the API field only, retaining official tokens and
    // unrelated native data. The resulting provider cannot fall back to them.
    std::fs::write(
        &path,
        "model='custom-id'\nopenai_base_url='https://relay.example'\n",
    )
    .unwrap();
    let read = native::read(&context, None).unwrap();
    assert!(read.credential.can_clear);
    native_edit::write(
        &context,
        &read,
        &read.configuration,
        &[],
        &ApiKeyChange::Clear,
        None,
    )
    .unwrap();
    let auth = native::read_json(&auth).unwrap();
    assert!(auth.get("OPENAI_API_KEY").is_none());
    assert_eq!(auth["tokens"]["access_token"], "preserved-token");
    assert_eq!(auth["unknown"], true);
    assert!(matches!(
        native::read(&context, None).unwrap().source,
        native::CredentialSource::Missing
    ));

    // Keeping an official login must not silently send its token to a new API URL.
    std::fs::write(&path, "model='native-default'\n").unwrap();
    let read = native::read(&context, None).unwrap();
    assert_eq!(read.observation.login_status, "signed_in");
    assert!(matches!(read.source, native::CredentialSource::Missing));
    let mut desired = read.configuration.clone();
    if let CustomApiConfiguration::Codex { base_url, mode, .. } = &mut desired {
        *base_url = "https://new-api.example".into();
        *mode = Some(ConnectionMode::CustomApi);
    }
    native_edit::write(
        &context,
        &read,
        &desired,
        &[FieldEdit {
            path: vec!["baseUrl".into()],
            before: json!(""),
            after: json!("https://new-api.example"),
            label: "接口地址".into(),
        }],
        &ApiKeyChange::Keep,
        None,
    )
    .unwrap();
    assert!(matches!(
        native::read(&context, Some(ConnectionMode::CustomApi))
            .unwrap()
            .source,
        native::CredentialSource::Missing
    ));

    // A full native catalog is edited without launching/scanning any executable.
    let full = json!({"models":[
        {"slug":"custom-a","display_name":"A","visibility":"list","context_window":4096,"input_modalities":["text"],"supports_reasoning_summary_parameter":false,"future_field":{"keep":true}},
        {"slug":"custom-b","display_name":"B","visibility":"list","context_window":8192},
        {"slug":"internal","display_name":"Internal","visibility":"hide","opaque":"keep"}
    ],"native_extension":{"keep":true}});
    let catalog_path = root.join("existing-catalog.json");
    std::fs::write(&catalog_path, serde_json::to_vec(&full).unwrap()).unwrap();
    let config_text = format!(
        "model_provider='relay'\nmodel='custom-a'\nmodel_catalog_json={}\n[model_providers.relay]\nbase_url='https://relay.example'\nenv_key='RELAY_KEY'\nwire_api='responses'\n",
        serde_json::to_string(&catalog_path).unwrap()
    );
    std::fs::write(&path, &config_text).unwrap();
    let current = native::read(&context, None).unwrap();
    assert!(
        current.configured_model_ids.is_none(),
        "an imported catalog is not a Rovai-maintained list"
    );
    let mut desired = current.configuration.clone();
    if let CustomApiConfiguration::Codex {
        models,
        default_model,
        default_row_id,
        ..
    } = &mut desired
    {
        *default_model = models[1].id.clone();
        *default_row_id = Some(models[1].row_id.clone());
    }
    let default_edit = FieldEdit {
        path: vec!["defaultRowId".into()],
        before: Value::Null,
        after: Value::Null,
        label: String::new(),
    };
    native_edit::write(
        &context,
        &current,
        &desired,
        &[default_edit],
        &ApiKeyChange::Keep,
        None,
    )
    .unwrap();
    let after_default = native::read(&context, None).unwrap();
    assert_eq!(after_default.catalog_path.as_ref(), Some(&catalog_path));
    assert_eq!(native::read_json(&catalog_path).unwrap(), full);
    assert_eq!(
        after_default.configuration.default_model(),
        Some("custom-b")
    );
    assert!(
        after_default
            .snapshot(&context, false)
            .model_is_configured("other-model")
    );
    if let CustomApiConfiguration::Codex { models, .. } = &mut desired {
        models[0].display_name = "Edited name".into();
    }
    let generated = codex_catalog::generate(None, &context, &after_default, &desired)
        .await
        .unwrap();
    let mut expected = full.clone();
    expected["models"][0]["display_name"] = json!("Edited name");
    expected["rovai_managed_model_list"] = json!(false);
    assert_eq!(generated, expected);
    let list_edit = FieldEdit {
        path: vec!["codexModels".into()],
        before: Value::Null,
        after: Value::Null,
        label: String::new(),
    };
    native_edit::write(
        &context,
        &after_default,
        &desired,
        &[list_edit],
        &ApiKeyChange::Keep,
        Some(&generated),
    )
    .unwrap();
    let saved = native::read(&context, None).unwrap();
    assert!(
        saved
            .snapshot(&context, false)
            .model_is_configured("custom-b")
    );
    assert!(
        saved
            .snapshot(&context, false)
            .model_is_configured("other-native-model"),
        "editing a label alone must not create an allowlist"
    );
    assert_eq!(
        native::read_json(saved.catalog_path.as_ref().unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        native::read_json(&catalog_path).unwrap(),
        full,
        "old files remain immutable for running processes"
    );

    let mut removed = desired.clone();
    if let CustomApiConfiguration::Codex { models, .. } = &mut removed {
        models.remove(0);
    }
    let generated = codex_catalog::generate(None, &context, &saved, &removed)
        .await
        .unwrap();
    let edit = FieldEdit {
        path: vec!["codexModels".into()],
        before: Value::Null,
        after: Value::Null,
        label: String::new(),
    };
    native_edit::write(
        &context,
        &saved,
        &removed,
        &[edit],
        &ApiKeyChange::Keep,
        Some(&generated),
    )
    .unwrap();
    let maintained = native::read(&context, None)
        .unwrap()
        .snapshot(&context, false);
    assert!(maintained.model_is_configured("custom-b"));
    assert!(!maintained.model_is_configured("custom-a"));

    let claude = native::NativeContext {
        kind: AdapterKind::ClaudeCodeCli,
        directory: root.join("claude"),
        artifact_root: root.join("claude-artifacts"),
        environment: BTreeMap::from([
            ("ANTHROPIC_AUTH_TOKEN".into(), "shell-only-key".into()),
            ("ANTHROPIC_BASE_URL".into(), "https://relay.example".into()),
            (
                "ANTHROPIC_CUSTOM_HEADERS".into(),
                "x-routing-tag: private-routing-tag\nProxy-Authorization: Basic independent-proxy-secret".into(),
            ),
        ]),
    };
    let snapshot = native::read(&claude, None)
        .unwrap()
        .snapshot(&claude, false);
    let mut command = tokio::process::Command::new("not-spawned");
    claude_native::configure(&snapshot, &mut command).unwrap();
    assert!(
        command
            .as_std()
            .get_envs()
            .any(|(name, value)| name == "ANTHROPIC_CUSTOM_HEADERS"
                && value == Some(std::ffi::OsStr::new("x-routing-tag: private-routing-tag\nProxy-Authorization: Basic independent-proxy-secret")))
    );
    assert!(
        !snapshot
            .claude_settings()
            .unwrap()
            .to_string()
            .contains("private-routing-tag")
    );
    private_storage::atomic_write_private_bytes(
        &claude.path(),
        br#"{"permissions":{"allow":["Read"]},"theme":"dark","mcpServers":{"unrelated":{}}}"#,
    )
    .unwrap();
    snapshot.assert_current().unwrap();
    assert_eq!(
        snapshot.identity().unwrap(),
        native::read(&claude, None)
            .unwrap()
            .snapshot(&claude, false)
            .identity()
            .unwrap()
    );
    let official_frozen = native::read(&claude, Some(ConnectionMode::OfficialLogin))
        .unwrap()
        .snapshot(&claude, true);
    private_storage::atomic_write_private_bytes(
        &claude.path(),
        br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"new-dormant-api-key"}}"#,
    )
    .unwrap();
    official_frozen.assert_current().unwrap();
    official_frozen.redactor().unwrap();
    assert_eq!(
        official_frozen.identity().unwrap(),
        native::read(&claude, Some(ConnectionMode::OfficialLogin))
            .unwrap()
            .snapshot(&claude, true)
            .identity()
            .unwrap()
    );
    private_storage::atomic_write_private_bytes(&claude.path(), b"{}").unwrap();
    let mut settings = json!({"effective": snapshot.claude_settings().unwrap()});
    let status = claude_native::Identity::from_initialize(
        &json!({"account":{"tokenSource":"ANTHROPIC_AUTH_TOKEN","apiProvider":"firstParty"}}),
    );
    claude_native::validate(&snapshot, &settings, &status, None).unwrap();
    settings["effective"]["env"]["ANTHROPIC_AUTH_TOKEN"] = json!("wrong-shell-key");
    assert!(claude_native::validate(&snapshot, &settings, &status, None).is_err());
    let mut official = snapshot.clone();
    official
        .configuration
        .set_mode(Some(ConnectionMode::OfficialLogin));
    for identity in [
        claude_native::Identity::Unknown,
        claude_native::Identity::SignedOut,
    ] {
        claude_native::validate_identity(&snapshot, &identity).unwrap();
        claude_native::validate_identity(&official, &identity).unwrap();
    }
    assert!(
        claude_native::validate_identity(&snapshot, &claude_native::Identity::Official).is_err()
    );
    assert!(claude_native::validate_identity(&official, &status).is_err());
    if let CustomApiConfiguration::ClaudeCode { models, .. } = &mut official.configuration {
        models.sonnet_model = "dormant-api-sonnet".into();
    }
    claude_native::validate(&official, &json!({"effective":official.claude_settings().unwrap(),"applied":{"model":"native-sonnet"}}), &claude_native::Identity::Official, Some("sonnet")).unwrap();

    // Key rotation preserves the effective authentication method, including shell sources.
    for (index, variable) in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .iter()
        .enumerate()
    {
        for shell in [false, true] {
            let mut context = claude.clone();
            context.directory = root.join(format!("rotation-{index}-{shell}"));
            context.environment = BTreeMap::from([
                ("HOME".into(), root.to_string_lossy().into_owned()),
                (
                    "CLAUDE_CODE_OAUTH_TOKEN".into(),
                    "fake-official-oauth".into(),
                ),
            ]);
            let mut native_doc = json!({"model":"top-model","env":{"ANTHROPIC_BASE_URL":"https://relay.example"},"unknown":{"keep":true}});
            if shell {
                context
                    .environment
                    .insert((*variable).into(), "old-static-key".into());
            } else {
                native_doc["env"][*variable] = json!("old-static-key");
            }
            private_storage::atomic_write_private_bytes(
                &context.path(),
                &serde_json::to_vec(&native_doc).unwrap(),
            )
            .unwrap();
            let read = native::read(&context, None).unwrap();
            assert_eq!(read.configuration.default_model(), Some("top-model"));
            let mut desired = read.configuration.clone();
            if let CustomApiConfiguration::ClaudeCode { models, .. } = &mut desired {
                models.model = "edited-top-model".into();
            }
            let edit = FieldEdit {
                path: vec!["claudeModels".into(), "model".into()],
                before: json!("top-model"),
                after: json!("edited-top-model"),
                label: String::new(),
            };
            native_edit::write(
                &context,
                &read,
                &desired,
                &[edit.clone()],
                &ApiKeyChange::Replace {
                    value: "rotated-static-key".into(),
                },
                None,
            )
            .unwrap();
            let doc = native::read_json(&context.path()).unwrap();
            assert_eq!(doc["env"][*variable], "rotated-static-key");
            assert_eq!(doc["model"], "edited-top-model");
            assert!(doc["env"].get("ANTHROPIC_MODEL").is_none());
            assert!(!doc.to_string().contains("fake-official-oauth"));
            let read = native::read(&context, Some(ConnectionMode::OfficialLogin)).unwrap();
            assert_eq!(read.observation.login_status, "signed_in");
            let snap = read.snapshot(&context, true);
            assert!(
                snap.claude_settings().unwrap()["env"]
                    .get("CLAUDE_CODE_OAUTH_TOKEN")
                    .is_none()
            );
            let mut command = Command::new("not-spawned");
            claude_native::configure_environment(&snap, &mut command).unwrap();
            assert!(
                !command
                    .as_std()
                    .get_envs()
                    .any(|(name, _)| name == "CLAUDE_CODE_OAUTH_TOKEN")
            );
            claude_native::validate(
                &snap,
                &json!({"effective":snap.claude_settings().unwrap()}),
                &claude_native::Identity::Official,
                None,
            )
            .unwrap();
            if let CustomApiConfiguration::ClaudeCode { models, .. } = &mut desired {
                models.model.clear();
            }
            native_edit::write(
                &context,
                &read,
                &desired,
                &[edit],
                &ApiKeyChange::Keep,
                None,
            )
            .unwrap();
            assert!(
                native::read_json(&context.path())
                    .unwrap()
                    .get("model")
                    .is_none()
            );
            assert_eq!(
                native::read(&context, None)
                    .unwrap()
                    .configuration
                    .default_model(),
                None
            );
        }
    }
    let mut login_context = claude.clone();
    login_context.directory = root.join("keychain-status");
    login_context.environment.clear();
    // Prevent this fixture from falling through to ambient process environment.
    login_context
        .environment
        .insert("HOME".into(), root.to_string_lossy().into_owned());
    assert_eq!(
        native::read(&login_context, None)
            .unwrap()
            .observation
            .login_status,
        "unknown"
    );
    native::record_claude_login(
        &json!({"loggedIn":true,"authMethod":"api_key"}),
        Some(&login_context.directory),
    );
    assert_eq!(
        native::read(&login_context, None)
            .unwrap()
            .observation
            .login_status,
        "unknown"
    );
    native::record_claude_login(
        &json!({"loggedIn":true,"authMethod":"claude.ai"}),
        Some(&login_context.directory),
    );
    assert_eq!(
        native::read(&login_context, None)
            .unwrap()
            .observation
            .login_status,
        "signed_in"
    );
    native::record_claude_login(
        &json!({"loggedIn":false,"authMethod":"none"}),
        Some(&login_context.directory),
    );
    assert_eq!(
        native::read(&login_context, None)
            .unwrap()
            .observation
            .login_status,
        "signed_out"
    );
    private_storage::atomic_write_private_bytes(&login_context.path(), b"{}\n").unwrap();
    assert_eq!(
        native::read(&login_context, None)
            .unwrap()
            .observation
            .login_status,
        "unknown",
        "external config changes invalidate native identity hints"
    );
    std::fs::remove_dir_all(root).unwrap();
}
