use super::*;
use crate::runtime_startup::RuntimeStartupConfiguration;
#[cfg(feature = "extended-tests")]
use crate::runtime_startup::{self, RuntimeEnvironmentVariable};
use std::collections::BTreeMap;
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
    for value in ["", "********", "bad\nkey", "  ***** \n", " \t \r\n"] {
        assert!(
            ApiKeyChange::Replace {
                value: value.into()
            }
            .validate()
            .is_err()
        );
    }
    ApiKeyChange::Replace {
        value: " \t trimmed-key \r\n".into(),
    }
    .validate()
    .unwrap();
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
    let preview = runtime_startup::resolve_draft(
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
        Some("isolated-new-key")
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
    let draft = runtime_startup::resolve_draft(
        &db,
        AdapterKind::CodexCli,
        draft,
        ApiKeyChange::Replace {
            value: "draft-auto-replacement".into(),
        },
    )
    .unwrap();
    let draft = draft.custom_api_snapshot.unwrap();
    assert!(draft.key().unwrap().is_none());
    assert!(matches!(
        draft.credential_source,
        native::CredentialSource::NativeManaged { .. }
    ));
    assert_eq!(draft.configuration.base_url(), "https://native.example");
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
    db.connection()
        .execute_batch("DROP TRIGGER reject_startup_update;")
        .unwrap();

    // Same transaction owner: switching on Save must ignore all hidden API input,
    // preserve OAuth, merge unrelated external fields and invalidate old sessions.
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli] {
        let context = native::NativeContext::resolve(
            kind,
            &RuntimeStartupConfiguration::default(),
            db.path(),
        )
        .unwrap();
        let auth_path = context.directory.join("auth.json");
        if kind == AdapterKind::ClaudeCodeCli {
            private_storage::atomic_write_private_bytes(&context.path(), br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"active-api-key","ANTHROPIC_BASE_URL":"https://old.example","ANTHROPIC_MODEL":"old-model","CLAUDE_CODE_OAUTH_TOKEN":"official-oauth","ANTHROPIC_CUSTOM_HEADERS":"Authorization: Bearer header-key\nX-Api-Key: header-api-key\nX-Trace: preserved\nProxy-Authorization: independent-proxy"},"unknown":true}"#).unwrap();
        } else {
            private_storage::atomic_write_private_bytes(&context.path(), b"model_provider='relay'\nmodel='api-model'\n[model_providers.relay]\nbase_url='https://old.example'\nexperimental_bearer_token='active-api-key'\n[model_providers.unrelated]\nbase_url='https://unrelated.example'\nexperimental_bearer_token='unrelated-key'\n").unwrap();
            private_storage::atomic_write_private_bytes(&auth_path, br#"{"auth_mode":"apikey","OPENAI_API_KEY":"fallback-api-key","tokens":{"access_token":"official-oauth","refresh_token":"official-refresh"},"unknown":true}"#).unwrap();
        }
        let saved = runtime_startup::load(&db, kind).unwrap();
        let before = std::fs::read(context.path()).unwrap();
        let before_auth = native::read_bytes(&auth_path).unwrap();
        let frozen = saved.configuration.custom_api_snapshot.as_ref().unwrap();
        let mut edits = vec![
            FieldEdit {
                path: vec!["mode".into()],
                before: json!("custom_api"),
                after: json!("official_login"),
                label: "连接方式".into(),
            },
            FieldEdit {
                path: vec!["nativeRevision".into()],
                before: json!(saved.native_revision),
                after: json!(saved.native_revision),
                label: "当前连接".into(),
            },
            FieldEdit {
                path: vec!["baseUrl".into()],
                before: json!("stale"),
                after: json!("not a url"),
                label: "隐藏地址".into(),
            },
            FieldEdit {
                path: vec!["credentialVersion".into()],
                before: json!("stale"),
                after: json!("replace"),
                label: "隐藏 Key".into(),
            },
        ];
        edits.push(FieldEdit {
            path: vec![
                if kind == AdapterKind::CodexCli {
                    "codexModels"
                } else {
                    "claudeModels"
                }
                .into(),
            ],
            before: Value::Null,
            after: json!(["invalid hidden model draft"]),
            label: "隐藏模型".into(),
        });
        let key = || ApiKeyChange::Replace {
            value: "hidden-invalid key\nnever-write".into(),
        };
        let prepared = runtime_startup::prepare_save(&db, kind, edits.clone(), &key()).unwrap();
        assert!(prepared.conflicts.is_empty());
        assert_eq!(prepared.edits.len(), 2);
        assert_eq!(
            std::fs::read(context.path()).unwrap(),
            before,
            "preparing a draft is read-only"
        );
        db.connection().execute_batch("CREATE TRIGGER reject_native_switch BEFORE INSERT ON runtime_startup_setting BEGIN SELECT RAISE(FAIL, 'isolated switch failure'); END;").unwrap();
        assert!(runtime_startup::commit_save(&mut db, kind, prepared, 4, key(), None).is_err());
        assert_eq!(std::fs::read(context.path()).unwrap(), before);
        assert_eq!(native::read_bytes(&auth_path).unwrap(), before_auth);
        db.connection()
            .execute_batch("DROP TRIGGER reject_native_switch;")
            .unwrap();
        // External unrelated content is merged, and does not conflict with a mode switch.
        if kind == AdapterKind::ClaudeCodeCli {
            let mut doc = native::read_json(&context.path()).unwrap();
            doc["external"] = json!("preserved");
            std::fs::write(context.path(), serde_json::to_vec(&doc).unwrap()).unwrap();
        } else {
            let mut doc = native::read_toml(&context.path()).unwrap();
            doc["model_providers"]["unrelated"]["name"] = toml_edit::value("external");
            std::fs::write(context.path(), doc.to_string()).unwrap();
        }
        let prepared = runtime_startup::prepare_save(&db, kind, edits, &key()).unwrap();
        assert!(prepared.conflicts.is_empty());
        let result = runtime_startup::commit_save(&mut db, kind, prepared, 5, key(), None).unwrap();
        assert!(result.native_written && result.reconnect_required);
        assert_eq!(
            result.configuration.custom_api.as_ref().unwrap().mode(),
            Some(ConnectionMode::OfficialLogin)
        );
        assert!(frozen.assert_current().is_err());
        assert!(
            !std::fs::read_to_string(context.path())
                .unwrap()
                .contains("never-write")
        );
        if kind == AdapterKind::ClaudeCodeCli {
            let doc = native::read_json(&context.path()).unwrap();
            assert_eq!(doc["env"]["CLAUDE_CODE_OAUTH_TOKEN"], "official-oauth");
            assert!(doc["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
            assert_eq!(
                doc["env"]["ANTHROPIC_CUSTOM_HEADERS"],
                "X-Trace: preserved\nProxy-Authorization: independent-proxy"
            );
            assert_eq!(doc["external"], "preserved");
        } else {
            let doc = native::read_toml(&context.path()).unwrap();
            assert_eq!(doc["model_provider"].as_str(), Some("openai"));
            assert!(doc.get("model").is_none());
            assert_eq!(
                doc["model_providers"]["unrelated"]["experimental_bearer_token"].as_str(),
                Some("unrelated-key")
            );
            let auth = native::read_json(&auth_path).unwrap();
            assert!(auth.get("OPENAI_API_KEY").is_none());
            assert_eq!(auth["auth_mode"], "chatgpt");
            assert_eq!(auth["tokens"]["refresh_token"], "official-refresh");
            assert_eq!(auth["unknown"], true);
        }
        // An external official switch conflicts with API edits, rather than silently dropping them.
        let conflict = runtime_startup::prepare_save(
            &db,
            kind,
            vec![
                FieldEdit {
                    path: vec!["mode".into()],
                    before: json!("custom_api"),
                    after: json!("custom_api"),
                    label: String::new(),
                },
                FieldEdit {
                    path: vec!["baseUrl".into()],
                    before: json!("https://old.example"),
                    after: json!("https://new.example"),
                    label: String::new(),
                },
            ],
            &ApiKeyChange::Keep,
        )
        .unwrap();
        assert!(conflict.conflicts.iter().any(|e| e.edit.path == ["mode"]));
        // Changing native configuration elsewhere overrides legacy UI metadata,
        // including values which the current native editor does not recognize.
        db.connection()
            .execute(
                "UPDATE runtime_startup_setting SET configuration_json=?1 WHERE runtime_kind=?2",
                rusqlite::params![
                    r#"{"programPath":null,"environment":[],"_connectionMode":"retired-value"}"#,
                    kind.as_str()
                ],
            )
            .unwrap();
        std::fs::write(context.path(), before).unwrap();
        assert!(
            runtime_startup::load(&db, kind)
                .unwrap()
                .configuration
                .custom_api
                .unwrap()
                .enabled()
        );
        let conflicted = runtime_startup::prepare_save(
            &db,
            kind,
            vec![
                FieldEdit {
                    path: vec!["mode".into()],
                    before: json!("custom_api"),
                    after: json!("official_login"),
                    label: String::new(),
                },
                FieldEdit {
                    path: vec!["nativeRevision".into()],
                    before: json!(result.native_revision),
                    after: json!(result.native_revision),
                    label: String::new(),
                },
            ],
            &ApiKeyChange::Keep,
        )
        .unwrap();
        assert!(
            conflicted
                .conflicts
                .iter()
                .any(|e| e.edit.path == ["nativeRevision"])
        );
    }
    // Official Save owns removal of old private startup entries, including a
    // referenced provider key. Read and preparation cannot remove them early.
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli] {
        let context = native::NativeContext::resolve(
            kind,
            &RuntimeStartupConfiguration::default(),
            db.path(),
        )
        .unwrap();
        let key_name = if kind == AdapterKind::CodexCli {
            "LEGACY_RELAY_KEY"
        } else {
            "ANTHROPIC_AUTH_TOKEN"
        };
        let base_name = if kind == AdapterKind::CodexCli {
            "OPENAI_BASE_URL"
        } else {
            "ANTHROPIC_BASE_URL"
        };
        let mut environment = vec![
            RuntimeEnvironmentVariable {
                name: key_name.into(),
                value: "legacy-private-key".into(),
            },
            RuntimeEnvironmentVariable {
                name: base_name.into(),
                value: "https://legacy.example".into(),
            },
            RuntimeEnvironmentVariable {
                name: "KEEP_ME".into(),
                value: "ordinary".into(),
            },
        ];
        if kind == AdapterKind::ClaudeCodeCli {
            environment.push(RuntimeEnvironmentVariable {
                name: "CLAUDE_CODE_OAUTH_TOKEN".into(),
                value: "official-private-token".into(),
            });
            std::fs::write(
                context.path(),
                br#"{"permissions":{"defaultMode":"default"},"model":"api-model"}"#,
            )
            .unwrap();
        } else {
            environment.push(RuntimeEnvironmentVariable {
                name: "OPENAI_API_KEY".into(),
                value: "legacy-fallback-key".into(),
            });
            std::fs::write(context.path(), b"model_provider='relay'\nmodel_providers={relay={base_url='https://legacy.example',env_key='LEGACY_RELAY_KEY'},other={name='keep'}}\n").unwrap();
            std::fs::write(context.directory.join("auth.json"), br#"{"auth_mode":"apikey","OPENAI_API_KEY":"file-api-key","tokens":{"access_token":"official-token","refresh_token":"official-refresh"}}"#).unwrap();
        }
        let legacy = RuntimeStartupConfiguration {
            environment,
            ..Default::default()
        };
        let revision = runtime_startup::load(&db, kind).unwrap().revision;
        runtime_startup::save(&mut db, kind, revision, legacy, 6).unwrap();
        let auth_path = context.directory.join("auth.json");
        #[cfg(unix)]
        for path in std::iter::once(context.path())
            .chain((kind == AdapterKind::CodexCli).then_some(auth_path.clone()))
        {
            let target = path.with_extension("native-target");
            std::fs::rename(&path, &target).unwrap();
            std::os::unix::fs::symlink(target.file_name().unwrap(), &path).unwrap();
        }
        let before = std::fs::read(context.path()).unwrap();
        let auth_before = native::read_bytes(&auth_path).unwrap();
        let saved = runtime_startup::load(&db, kind).unwrap();
        let public = serde_json::to_string(&runtime_startup::public(saved.clone())).unwrap();
        assert!(
            !public.contains("legacy-private-key") && !public.contains("official-private-token")
        );
        let edits = vec![FieldEdit {
            path: vec!["mode".into()],
            before: json!("custom_api"),
            after: json!("official_login"),
            label: String::new(),
        }];
        let prepared =
            runtime_startup::prepare_save(&db, kind, edits.clone(), &ApiKeyChange::Keep).unwrap();
        assert!(
            !prepared
                .configuration
                .environment
                .iter()
                .any(|e| e.name == key_name || e.name == base_name || e.name == "OPENAI_API_KEY")
        );
        assert!(
            runtime_startup::load(&db, kind)
                .unwrap()
                .configuration
                .environment
                .iter()
                .any(|e| e.name == key_name)
        );
        assert_eq!(std::fs::read(context.path()).unwrap(), before);
        db.connection().execute_batch("CREATE TRIGGER reject_legacy_clear BEFORE INSERT ON runtime_startup_setting BEGIN SELECT RAISE(FAIL, 'isolated failure'); END;").unwrap();
        assert!(
            runtime_startup::commit_save(&mut db, kind, prepared, 7, ApiKeyChange::Keep, None)
                .is_err()
        );
        assert_eq!(std::fs::read(context.path()).unwrap(), before);
        assert_eq!(native::read_bytes(&auth_path).unwrap(), auth_before);
        assert!(
            runtime_startup::load(&db, kind)
                .unwrap()
                .configuration
                .environment
                .iter()
                .any(|e| e.name == key_name)
        );
        db.connection()
            .execute_batch("DROP TRIGGER reject_legacy_clear;")
            .unwrap();
        let prepared =
            runtime_startup::prepare_save(&db, kind, edits, &ApiKeyChange::Keep).unwrap();
        let saved =
            runtime_startup::commit_save(&mut db, kind, prepared, 8, ApiKeyChange::Keep, None)
                .unwrap();
        assert_eq!(
            saved.configuration.custom_api.as_ref().unwrap().mode(),
            Some(ConnectionMode::OfficialLogin)
        );
        assert!(
            saved
                .configuration
                .environment
                .iter()
                .any(|e| e.name == "KEEP_ME")
        );
        assert!(
            !saved
                .configuration
                .environment
                .iter()
                .any(|e| e.name == key_name || e.name == base_name || e.name == "OPENAI_API_KEY")
        );
        if kind == AdapterKind::ClaudeCodeCli {
            assert!(saved.configuration.environment.iter().any(|e| e.name
                == "CLAUDE_CODE_OAUTH_TOKEN"
                && e.value == "official-private-token"));
        } else {
            let auth = native::read_json(&auth_path).unwrap();
            assert!(auth.get("OPENAI_API_KEY").is_none());
            assert_eq!(auth["tokens"]["refresh_token"], "official-refresh");
        }
        #[cfg(unix)]
        for path in std::iter::once(context.path())
            .chain((kind == AdapterKind::CodexCli).then_some(auth_path.clone()))
        {
            assert!(
                path.symlink_metadata().unwrap().file_type().is_symlink(),
                "both commit and rollback preserve native links"
            );
        }
    }

    // Optional native observation cannot turn an unreadable source into a new execution gate.
    db.connection().execute(
        "UPDATE runtime_startup_setting SET configuration_json=?1 WHERE runtime_kind='codex-cli'",
        [r#"{"programPath":null,"environment":[{"name":"CODEX_HOME","value":"relative-native-directory"}]}"#],
    ).unwrap();
    assert!(
        runtime_startup::load(&db, AdapterKind::CodexCli)
            .unwrap()
            .connection_read_error
            .is_some()
    );
    assert!(
        runtime_startup::snapshot_from_connection(db.connection(), AdapterKind::CodexCli)
            .unwrap()
            .is_none()
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
    assert_eq!(snapshot.key().unwrap().as_deref(), Some("replacement-key"));
    assert!(
        !context.artifact_root.exists(),
        "reading native configuration creates no launch overlay"
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
    let read = native::read(&context, None).unwrap();
    let provider_before = doc["model_providers"]["relay"].clone();
    let mut desired = read.configuration.clone();
    if let CustomApiConfiguration::Codex { default_model, .. } = &mut desired {
        *default_model = "different-default".into();
    }
    native_edit::write(
        &context,
        &read,
        &desired,
        &[FieldEdit {
            path: vec!["defaultRowId".into()],
            before: Value::Null,
            after: Value::Null,
            label: String::new(),
        }],
        &ApiKeyChange::Keep,
        None,
    )
    .unwrap();
    assert_eq!(
        native::read_toml(&path).unwrap()["model_providers"]["relay"].to_string(),
        provider_before.to_string(),
        "model edits preserve transport, proxy auth, query parameters and timeouts"
    );

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
    let auto_api = native::read(&context, None).unwrap();
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
    assert!(
        frozen_official.assert_current().is_err(),
        "a stale display mode cannot hide native changes"
    );
    frozen_official.redactor().unwrap();
    std::fs::write(&auth, serde_json::to_vec(&fallback).unwrap()).unwrap();
    // Auto uses the native file fallback without promoting it over a possible keyring identity.
    assert!(auto.configuration.enabled());
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

    // Native TOML tables and inline tables are equivalent; edit only selected
    // fields without converting or dropping unrelated profile/provider content.
    for (index, tables) in [
        "model_providers={relay={base_url='https://inline.example',env_key='RELAY_KEY',query_params={version='v1'}},other={name='untouched'}}\nprofiles={work={model_provider='relay',model='a',unknown='keep'},other={model='leave'}}\n",
        "[model_providers]\nrelay={base_url='https://inline.example',env_key='RELAY_KEY',query_params={version='v1'}}\nother={name='untouched'}\n[profiles]\nwork={model_provider='relay',model='a',unknown='keep'}\nother={model='leave'}\n",
    ].iter().enumerate() {
        let mut inline = context.clone();
        inline.directory = root.join(format!("inline-{index}"));
        private_storage::atomic_write_private_bytes(&inline.path(), format!("# retain comment\nprofile='work'\n{tables}").as_bytes()).unwrap();
        let read = native::read(&inline, None).unwrap();
        let mut desired = read.configuration.clone();
        if let CustomApiConfiguration::Codex { base_url, default_model, .. } = &mut desired {
            *base_url = "https://edited.example/prefix".into();
            *default_model = "b".into();
        }
        let edits = ["baseUrl", "defaultRowId"].map(|name| FieldEdit { path: vec![name.into()], before: Value::Null, after: Value::Null, label: String::new() });
        native_edit::write(&inline, &read, &desired, &edits, &ApiKeyChange::Replace { value: " \t padded-key \r\n".into() }, None).unwrap();
        let text = std::fs::read_to_string(inline.path()).unwrap();
        assert!(text.starts_with("# retain comment"));
        let doc: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(doc["model_providers"]["relay"]["experimental_bearer_token"].as_str(), Some("padded-key"));
        assert_eq!(doc["model_providers"]["relay"]["query_params"]["version"].as_str(), Some("v1"));
        assert_eq!(doc["model_providers"]["other"]["name"].as_str(), Some("untouched"));
        assert_eq!(doc["profiles"]["work"]["unknown"].as_str(), Some("keep"));
        assert_eq!(doc["profiles"]["work"]["model"].as_str(), Some("b"));
        assert_eq!(doc["profiles"]["other"]["model"].as_str(), Some("leave"));
        let read = native::read(&inline, None).unwrap();
        desired.set_mode(Some(ConnectionMode::OfficialLogin));
        native_edit::write(&inline, &read, &desired, &[FieldEdit { path: vec!["mode".into()], before: json!("custom_api"), after: json!("official_login"), label: String::new() }], &ApiKeyChange::Keep, None).unwrap();
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(inline.path()).unwrap()).unwrap();
        assert_eq!(doc["profiles"]["work"]["model_provider"].as_str(), Some("openai"));
        assert_eq!(doc["profiles"]["work"]["unknown"].as_str(), Some("keep"));
        assert_eq!(doc["profiles"]["other"]["model"].as_str(), Some("leave"));
    }
    // An unrecognized native credential mechanism does not authorize rebuilding
    // the existing provider or inserting an unset Key reference during model edits.
    let mut opaque = context.clone();
    opaque.directory = root.join("opaque-credential");
    opaque.environment = BTreeMap::from([("HOME".into(), root.to_string_lossy().into_owned())]);
    private_storage::atomic_write_private_bytes(
        &opaque.path(),
        b"openai_base_url='https://opaque.example'\nmodel='a'\nfuture_auth='native-source'\n",
    )
    .unwrap();
    let read = native::read(&opaque, None).unwrap();
    assert_eq!(read.credential.status, "missing");
    let mut desired = read.configuration.clone();
    if let CustomApiConfiguration::Codex { default_model, .. } = &mut desired {
        *default_model = "b".into();
    }
    native_edit::write(
        &opaque,
        &read,
        &desired,
        &[FieldEdit {
            path: vec!["defaultRowId".into()],
            before: Value::Null,
            after: Value::Null,
            label: String::new(),
        }],
        &ApiKeyChange::Keep,
        None,
    )
    .unwrap();
    let doc = native::read_toml(&opaque.path()).unwrap();
    assert_eq!(doc["model"].as_str(), Some("b"));
    assert_eq!(doc["future_auth"].as_str(), Some("native-source"));
    assert!(doc.get("model_providers").is_none() && doc.get("model_provider").is_none());
    // An external shell Key remains external: official Save reports it and leaves
    // native bytes intact rather than attempting an execution-time override.
    opaque
        .environment
        .insert("OPENAI_API_KEY".into(), "external-key".into());
    let read = native::read(&opaque, None).unwrap();
    desired.set_mode(Some(ConnectionMode::OfficialLogin));
    let before = std::fs::read(opaque.path()).unwrap();
    let error = native_edit::write(
        &opaque,
        &read,
        &desired,
        &[FieldEdit {
            path: vec!["mode".into()],
            before: json!("custom_api"),
            after: json!("official_login"),
            label: String::new(),
        }],
        &ApiKeyChange::Keep,
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("OPENAI_API_KEY") && !error.contains("external-key"));
    assert_eq!(std::fs::read(opaque.path()).unwrap(), before);
    #[cfg(unix)]
    {
        use super::native_file::NativeFile;
        use std::os::unix::fs::PermissionsExt;
        let linked = root.join("linked-config");
        let first = root.join("first-target");
        let second = root.join("second-target");
        std::fs::write(&first, b"before").unwrap();
        std::fs::write(&second, b"before").unwrap();
        std::os::unix::fs::symlink(first.file_name().unwrap(), &linked).unwrap();
        let file = NativeFile::read(&linked).unwrap();
        std::fs::remove_file(&linked).unwrap();
        std::os::unix::fs::symlink(second.file_name().unwrap(), &linked).unwrap();
        assert!(
            file.write(b"must-not-write").is_err(),
            "retargeting conflicts even when bytes match"
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"before");
        assert_eq!(std::fs::read(&second).unwrap(), b"before");
        let shared = root.join("shared-dotfiles");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
        let missing = shared.join("missing-target");
        let dangling = root.join("dangling-link");
        std::os::unix::fs::symlink(missing.strip_prefix(&root).unwrap(), &dangling).unwrap();
        NativeFile::read(&dangling).unwrap().write(b"new").unwrap();
        assert!(
            dangling
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&missing).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(&missing).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let written = NativeFile::read(&dangling).unwrap();
        written.write(b"our-write").unwrap();
        std::fs::write(&missing, b"external-write").unwrap();
        assert!(written.restore(&Some(b"our-write".to_vec())).is_err());
        assert_eq!(std::fs::read(&missing).unwrap(), b"external-write");
    }

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
    assert_eq!(
        snapshot.credential_source.claude_variable(),
        "ANTHROPIC_AUTH_TOKEN"
    );
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
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
    // A requested display mode does not alter which native configuration is active.
    assert!(
        native::read(&claude, Some(ConnectionMode::OfficialLogin))
            .unwrap()
            .configuration
            .enabled()
    );

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
