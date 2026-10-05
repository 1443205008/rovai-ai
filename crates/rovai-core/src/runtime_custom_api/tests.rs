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
        (json!({"type":"apiKey"}), codex_native::Identity::Api),
        (json!({"type":"chatgpt"}), codex_native::Identity::Official),
        (Value::Null, codex_native::Identity::SignedOut),
        (json!({"type":"future"}), codex_native::Identity::Unknown),
    ] {
        assert_eq!(
            codex_native::Identity::from_account(
                &json!({"account":account,"requiresOpenaiAuth":true})
            ),
            expected
        );
    }
    assert_eq!(
        codex_native::Identity::from_account(&json!({"account":null,"requiresOpenaiAuth":false})),
        codex_native::Identity::Unknown
    );
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

    // A cloud route without an explicit endpoint still permits model-only edits.
    let cloud_kind = AdapterKind::ClaudeCodeCli;
    let saved = runtime_startup::load(&db, cloud_kind).unwrap();
    let cloud_context =
        native::NativeContext::resolve(cloud_kind, &saved.configuration, db.path()).unwrap();
    std::fs::write(
        cloud_context.path(),
        br#"{"env":{"CLAUDE_CODE_USE_BEDROCK":"1","ANTHROPIC_MODEL":"cloud-a"}}"#,
    )
    .unwrap();
    let cloud_saved = runtime_startup::load(&db, cloud_kind).unwrap();
    assert_eq!(
        cloud_saved
            .configuration
            .custom_api
            .as_ref()
            .unwrap()
            .base_url(),
        ""
    );
    let prepared = runtime_startup::prepare_save(
        &db,
        cloud_kind,
        vec![FieldEdit {
            path: vec!["claudeModels".into(), "model".into()],
            before: json!("cloud-a"),
            after: json!("cloud-b"),
            label: String::new(),
        }],
        &ApiKeyChange::Keep,
    )
    .unwrap();
    runtime_startup::commit_save(&mut db, cloud_kind, prepared, 9, ApiKeyChange::Keep, None)
        .unwrap();
    let cloud_doc = native::read_json(&cloud_context.path()).unwrap();
    assert_eq!(cloud_doc["env"]["CLAUDE_CODE_USE_BEDROCK"], "1");
    assert_eq!(cloud_doc["env"]["ANTHROPIC_MODEL"], "cloud-b");

    // Replacing a legacy Rovai-owned reference retires that hidden Key, rather
    // than exposing it as an ordinary variable after provider.env_key is removed.
    let kind = AdapterKind::CodexCli;
    let saved = runtime_startup::load(&db, kind).unwrap();
    let mut startup = saved.configuration;
    startup.environment = vec![RuntimeEnvironmentVariable {
        name: "PRIVATE_RELAY_CREDENTIAL".into(),
        value: "retired-fixture-key".into(),
    }];
    runtime_startup::save(&mut db, kind, saved.revision, startup, 10).unwrap();
    let settings = runtime_startup::load(&db, kind).unwrap();
    let context = native::NativeContext::resolve(kind, &settings.configuration, db.path()).unwrap();
    std::fs::write(context.path(), b"model='a'\nmodel_provider='relay'\n[model_providers.relay]\nbase_url='https://relay.example'\nenv_key='PRIVATE_RELAY_CREDENTIAL'\n").unwrap();
    let saved = runtime_startup::load(&db, kind).unwrap();
    let key = ApiKeyChange::Replace {
        value: "replacement-fixture-key".into(),
    };
    let prepared = runtime_startup::prepare_save(
        &db,
        kind,
        vec![FieldEdit {
            path: vec!["credentialVersion".into()],
            before: json!(saved.credential.as_ref().unwrap().version),
            after: json!("replace"),
            label: String::new(),
        }],
        &key,
    )
    .unwrap();
    let saved = runtime_startup::commit_save(&mut db, kind, prepared, 11, key, None).unwrap();
    assert!(
        saved
            .configuration
            .environment
            .iter()
            .all(|entry| entry.name != "PRIVATE_RELAY_CREDENTIAL")
    );
    let public = serde_json::to_string(&runtime_startup::public(saved)).unwrap();
    assert!(!public.contains("retired-fixture-key") && !public.contains("replacement-fixture-key"));

    // Identity observations never redefine file CAS, even across a failed refresh.
    let kind = AdapterKind::CodexCli;
    let mut current = runtime_startup::load(&db, kind).unwrap();
    current.configuration.environment.clear();
    runtime_startup::save(&mut db, kind, current.revision, current.configuration, 10).unwrap();
    let context =
        native::NativeContext::resolve(kind, &RuntimeStartupConfiguration::default(), db.path())
            .unwrap();
    std::fs::write(context.path(), b"cli_auth_credentials_store='keyring'\nmodel='native-id'\nmodel_catalog_json='catalog.json'\n").unwrap();
    let catalog = json!({"models":[{"slug":"native-id","display_name":"Before","visibility":"list","supported_in_api":true,"hidden_extension":"preserve"}]});
    private_storage::atomic_write_private_bytes(
        &context.directory.join("catalog.json"),
        &serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    codex_native::observe_fixture(&context, codex_native::Identity::Api);
    let saved = runtime_startup::load(&db, kind).unwrap();
    assert_eq!(
        saved.configuration.custom_api.as_ref().unwrap().mode(),
        None
    );
    assert_eq!(
        saved.connection_observation.as_ref().unwrap().initial_mode,
        Some(ConnectionMode::CustomApi)
    );
    let row = native::row_id("native-id").unwrap();
    let edits = vec![
        FieldEdit {
            path: vec!["codexModels".into(), row, "displayName".into()],
            before: json!("Before"),
            after: json!("After"),
            label: String::new(),
        },
        FieldEdit {
            path: vec!["nativeRevision".into()],
            before: json!(saved.native_revision),
            after: json!(saved.native_revision),
            label: String::new(),
        },
    ];
    codex_native::observe_fixture(&context, codex_native::Identity::Unknown);
    let latest = runtime_startup::load(&db, kind).unwrap();
    assert_eq!(latest.native_revision, saved.native_revision);
    assert_eq!(
        latest.connection_observation.as_ref().unwrap().login_status,
        "unknown"
    );
    assert_eq!(
        latest.connection_observation.as_ref().unwrap().initial_mode,
        Some(ConnectionMode::CustomApi),
        "last confirmed choice stays a display hint"
    );
    assert_eq!(latest.credential.as_ref().unwrap().status, "available");
    let prepared =
        runtime_startup::prepare_save(&db, kind, edits.clone(), &ApiKeyChange::Keep).unwrap();
    assert!(
        prepared.conflicts.is_empty(),
        "identity failure is not a native edit"
    );
    let mut renamed = catalog.clone();
    renamed["models"][0]["display_name"] = json!("After");
    // The executable/catalog step can yield. An external metadata edit during
    // that interval must not be overwritten by a stale generated catalog.
    let mut external_catalog = catalog.clone();
    external_catalog["models"][0]["external_capability"] = json!({"limit": 8192});
    let catalog_path = context.directory.join("catalog.json");
    std::fs::write(
        &catalog_path,
        serde_json::to_vec(&external_catalog).unwrap(),
    )
    .unwrap();
    let config_before = std::fs::read(context.path()).unwrap();
    let error = runtime_startup::commit_save(
        &mut db,
        kind,
        prepared,
        11,
        ApiKeyChange::Keep,
        Some(&renamed),
    )
    .unwrap_err();
    assert!(error.to_string().contains("模型目录在生成期间变化"));
    assert_eq!(std::fs::read(context.path()).unwrap(), config_before);
    assert_eq!(native::read_json(&catalog_path).unwrap(), external_catalog);
    let prepared =
        runtime_startup::prepare_save(&db, kind, edits.clone(), &ApiKeyChange::Keep).unwrap();
    let mut renamed = external_catalog;
    renamed["models"][0]["display_name"] = json!("After");
    let saved = runtime_startup::commit_save(
        &mut db,
        kind,
        prepared,
        11,
        ApiKeyChange::Keep,
        Some(&renamed),
    )
    .unwrap();
    assert!(saved.native_written);
    assert!(
        !saved.reconnect_required,
        "a display label does not change execution identity"
    );
    assert_eq!(
        saved.configuration.custom_api.as_ref().unwrap().mode(),
        None
    );
    assert_eq!(
        saved.connection_observation.as_ref().unwrap().login_status,
        "unknown"
    );
    assert_eq!(
        native::read_toml(&context.path()).unwrap()["cli_auth_credentials_store"].as_str(),
        Some("keyring")
    );
    let written_catalog =
        native::read_json(&native::read(&context, None).unwrap().catalog_path.unwrap()).unwrap();
    assert_eq!(
        written_catalog["models"][0]["external_capability"],
        json!({"limit": 8192})
    );
    let mut external = native::read_toml(&context.path()).unwrap();
    external["model_provider"] = toml_edit::value("other");
    std::fs::write(context.path(), external.to_string().as_bytes()).unwrap();
    let prepared = runtime_startup::prepare_save(&db, kind, edits, &ApiKeyChange::Keep).unwrap();
    assert!(
        prepared
            .conflicts
            .iter()
            .any(|c| c.edit.path == ["nativeRevision"]),
        "a real provider switch still conflicts"
    );

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
        launcher: None,
        codex_source: None,
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
    let auth_before = std::fs::read(&auth).unwrap();
    native_edit::write(
        &context,
        &read,
        &changed,
        &[FieldEdit {
            path: vec!["baseUrl".into()],
            before: json!("https://relay.example"),
            after: json!("https://new-api.example"),
            label: "接口地址".into(),
        }],
        &ApiKeyChange::Keep,
        None,
    )
    .unwrap();
    let doc = native::read_toml(&path).unwrap();
    assert_eq!(
        doc["openai_base_url"].as_str(),
        Some("https://new-api.example")
    );
    assert_eq!(doc["cli_auth_credentials_store"].as_str(), Some("keyring"));
    assert!(doc.get("model_providers").is_none() && doc.get("model_provider").is_none());
    assert_eq!(
        std::fs::read(&auth).unwrap(),
        auth_before,
        "changing an existing API address does not migrate credentials"
    );

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
    assert_eq!(
        auto.configuration.mode(),
        None,
        "an auto fallback file cannot identify the selected keyring account"
    );
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
    let original_official = std::fs::read(&path).unwrap();
    let error = native_edit::write(
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
    .unwrap_err();
    assert!(error.to_string().contains("官方登录凭据"));
    assert_eq!(std::fs::read(&path).unwrap(), original_official);
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
    assert_eq!(
        after_default.snapshot(&context, false).identity().unwrap(),
        saved.snapshot(&context, false).identity().unwrap(),
        "the generated label-only catalog must preserve process compatibility"
    );
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
        inline.codex_source = Some(codex_source::Source { base: inline.path(), legacy: true, ..Default::default() });
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
    // Address-only edits retain unknown native auth fields and provider selection.
    for (index, kind, original) in [
        (
            0,
            AdapterKind::CodexCli,
            "openai_base_url='https://before.example'\nmodel='a'\nfuture_auth={mechanism='native'}\n",
        ),
        (
            1,
            AdapterKind::CodexCli,
            "model_provider='relay'\nmodel_providers={relay={base_url='https://before.example',wire_api='responses',future_auth={mechanism='native'},query_params={tag='keep'}}}\n",
        ),
        (
            2,
            AdapterKind::ClaudeCodeCli,
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://before.example"},"futureAuth":{"mechanism":"native"}}"#,
        ),
    ] {
        let mut connection = opaque.clone();
        connection.kind = kind;
        connection.directory = root.join(format!("unknown-address-{index}"));
        private_storage::atomic_write_private_bytes(&connection.path(), original.as_bytes())
            .unwrap();
        let read = native::read(&connection, None).unwrap();
        assert_eq!(read.credential.status, "missing");
        let mut desired = read.configuration.clone();
        match &mut desired {
            CustomApiConfiguration::ClaudeCode { base_url, .. }
            | CustomApiConfiguration::Codex { base_url, .. } => {
                *base_url = "https://after.example".into()
            }
        }
        native_edit::write(
            &connection,
            &read,
            &desired,
            &[FieldEdit {
                path: vec!["baseUrl".into()],
                before: json!("https://before.example"),
                after: json!("https://after.example"),
                label: String::new(),
            }],
            &ApiKeyChange::Keep,
            None,
        )
        .unwrap();
        let parse = |text: &str| -> Value {
            if kind == AdapterKind::ClaudeCodeCli {
                serde_json::from_str(text).unwrap()
            } else {
                serde_json::to_value(toml::from_str::<toml::Value>(text).unwrap()).unwrap()
            }
        };
        assert_eq!(
            parse(&std::fs::read_to_string(connection.path()).unwrap()),
            parse(&original.replace("https://before.example", "https://after.example"))
        );
    }
    // Personal login restrictions can be removed with an explicit selection.
    // Managed files and OAuth stay native-owned, and an official account is
    // never promoted into a Key source for a newly selected API.
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli] {
        let mut personal = opaque.clone();
        personal.kind = kind;
        personal.directory = root.join(format!("personal-login-{}", kind.as_str()));
        let (initial, managed_name, managed_contents) = if kind == AdapterKind::ClaudeCodeCli {
            (
                r#"{"forceLoginMethod":"console","env":{"ANTHROPIC_BASE_URL":"https://before.example","ANTHROPIC_AUTH_TOKEN":"api-key","CLAUDE_CODE_OAUTH_TOKEN":"official-token"},"permissions":{"defaultMode":"default"}}"#,
                "managed-settings.json",
                r#"{"forceLoginMethod":"console"}"#,
            )
        } else {
            (
                "forced_login_method='api'\nmodel_provider='relay'\nmodel_providers={relay={base_url='https://before.example',experimental_bearer_token='api-key'}}\napproval_policy='on-request'\n",
                "requirements.toml",
                "allowed_approval_policies=['on-request']\n",
            )
        };
        private_storage::atomic_write_private_bytes(&personal.path(), initial.as_bytes()).unwrap();
        let managed = personal.directory.join(managed_name);
        std::fs::write(&managed, managed_contents).unwrap();
        let auth = personal
            .directory
            .join(if kind == AdapterKind::ClaudeCodeCli {
                ".credentials.json"
            } else {
                "auth.json"
            });
        let oauth = if kind == AdapterKind::ClaudeCodeCli {
            r#"{"claudeAiOauth":{"accessToken":"official-token"}}"#
        } else {
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"official-token","refresh_token":"official-refresh"}}"#
        };
        std::fs::write(&auth, oauth).unwrap();
        let read = native::read(&personal, None).unwrap();
        let mut official = read.configuration.clone();
        official.set_mode(Some(ConnectionMode::OfficialLogin));
        let mode = FieldEdit {
            path: vec!["mode".into()],
            before: json!("custom_api"),
            after: json!("official_login"),
            label: String::new(),
        };
        native_edit::write(
            &personal,
            &read,
            &official,
            &[mode],
            &ApiKeyChange::Keep,
            None,
        )
        .unwrap();
        if kind == AdapterKind::ClaudeCodeCli {
            let mut doc = native::read_json(&personal.path()).unwrap();
            assert!(doc.get("forceLoginMethod").is_none());
            assert_eq!(doc["env"]["CLAUDE_CODE_OAUTH_TOKEN"], "official-token");
            assert_eq!(doc["permissions"]["defaultMode"], "default");
            doc["forceLoginMethod"] = json!("claudeai");
            std::fs::write(personal.path(), serde_json::to_vec(&doc).unwrap()).unwrap();
        } else {
            let mut doc = native::read_toml(&personal.path()).unwrap();
            assert!(doc.get("forced_login_method").is_none());
            assert_eq!(doc["approval_policy"].as_str(), Some("on-request"));
            doc["forced_login_method"] = toml_edit::value("chatgpt");
            // Opaque auto/keyring identity may be displayed as reusable for the
            // native connection; it still cannot supply a new API's Key.
            doc["cli_auth_credentials_store"] = toml_edit::value("auto");
            std::fs::write(personal.path(), doc.to_string()).unwrap();
        }
        let read = native::read(&personal, None).unwrap();
        assert_eq!(
            read.configuration.mode(),
            Some(ConnectionMode::OfficialLogin)
        );
        let before = std::fs::read(personal.path()).unwrap();
        let api = configuration(kind);
        let edits = [
            FieldEdit {
                path: vec!["mode".into()],
                before: json!("official_login"),
                after: json!("custom_api"),
                label: String::new(),
            },
            FieldEdit {
                path: vec!["baseUrl".into()],
                before: json!(""),
                after: json!(api.base_url()),
                label: String::new(),
            },
        ];
        let error = native_edit::write(&personal, &read, &api, &edits, &ApiKeyChange::Keep, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("官方登录凭据"));
        assert_eq!(std::fs::read(personal.path()).unwrap(), before);
        native_edit::write(
            &personal,
            &read,
            &api,
            &edits,
            &ApiKeyChange::Replace {
                value: "new-api-key".into(),
            },
            None,
        )
        .unwrap();
        if kind == AdapterKind::ClaudeCodeCli {
            let doc = native::read_json(&personal.path()).unwrap();
            assert!(doc.get("forceLoginMethod").is_none());
            assert_eq!(doc["env"]["ANTHROPIC_AUTH_TOKEN"], "new-api-key");
            assert_eq!(doc["env"]["CLAUDE_CODE_OAUTH_TOKEN"], "official-token");
        } else {
            let doc = native::read_toml(&personal.path()).unwrap();
            assert!(doc.get("forced_login_method").is_none());
            assert_eq!(
                doc["model_providers"]["rovai_custom"]["experimental_bearer_token"].as_str(),
                Some("new-api-key")
            );
        }
        assert_eq!(std::fs::read_to_string(&auth).unwrap(), oauth);
        assert_eq!(std::fs::read_to_string(&managed).unwrap(), managed_contents);
    }
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
        std::fs::set_permissions(&missing, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(
            NativeFile::read(&dangling)
                .unwrap()
                .write(b"must-not-write")
                .unwrap_err()
                .to_string()
                .contains("只读")
        );
        assert_eq!(std::fs::read(&missing).unwrap(), b"external-write");
        assert_eq!(
            std::fs::metadata(&missing).unwrap().permissions().mode() & 0o777,
            0o400
        );
        std::fs::set_permissions(&missing, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let claude = native::NativeContext {
        kind: AdapterKind::ClaudeCodeCli,
        directory: root.join("claude"),
        artifact_root: root.join("claude-artifacts"),
            launcher: None,
            codex_source: None,
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
    // A fixed model is a field restriction, not a switch that clears all local
    // fields. Address and Key edits still reach the confirmed native source.
    let mut limited = context.clone();
    limited.directory = root.join("field-limits");
    let limited_path = limited.path();
    private_storage::atomic_write_private_bytes(&limited_path, b"model='native-default'\nmodel_provider='relay'\n[model_providers.relay]\nbase_url='https://before.example'\nexperimental_bearer_token='before-key'\nrequest_max_retries=2\n").unwrap();
    limited.codex_source = Some(codex_source::Source {
        base: limited_path.clone(),
        field_overrides: BTreeMap::from([
            ("model".into(), "sessionFlags".into()),
            ("model_provider".into(), "sessionFlags".into()),
        ]),
        selected_provider: Some("relay".into()),
        read_error: Some("原生来源暂未确认，已读取的配置仍保留。".into()),
        ..Default::default()
    });
    let before = native::read(&limited, None).unwrap();
    assert!(
        before
            .observation
            .conflict
            .as_deref()
            .unwrap()
            .contains("默认模型")
    );
    let mut desired = before.configuration.clone();
    if let CustomApiConfiguration::Codex { base_url, .. } = &mut desired {
        *base_url = "https://after.example".into();
    }
    let address_edit = FieldEdit {
        path: vec!["baseUrl".into()],
        before: json!("https://before.example"),
        after: json!("https://after.example"),
        label: String::new(),
    };
    let key_edit = FieldEdit {
        path: vec!["credentialVersion".into()],
        before: Value::Null,
        after: json!("replace"),
        label: String::new(),
    };
    native_edit::write(
        &limited,
        &before,
        &desired,
        &[address_edit.clone(), key_edit],
        &ApiKeyChange::Replace {
            value: "after-key".into(),
        },
        None,
    )
    .unwrap();
    let after = native::read(&limited, None).unwrap();
    assert_eq!(after.configuration.base_url(), "https://after.example");
    assert_eq!(
        after.snapshot(&limited, false).key().unwrap().as_deref(),
        Some("after-key")
    );
    assert_eq!(
        native::read_toml(&limited_path).unwrap()["model"].as_str(),
        Some("native-default")
    );
    let bytes = std::fs::read(&limited_path).unwrap();
    let error = native_edit::write(
        &limited,
        &after,
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
    .unwrap_err();
    assert!(error.to_string().contains("默认模型"));
    assert_eq!(std::fs::read(&limited_path).unwrap(), bytes);
    limited.codex_source.as_mut().unwrap().selected_provider = Some("other".into());
    assert!(
        native_edit::write(
            &limited,
            &after,
            &desired,
            &[address_edit.clone()],
            &ApiKeyChange::Keep,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("model_provider"),
        "a forced selection of another provider makes the local provider's URL ineffective"
    );
    assert_eq!(std::fs::read(&limited_path).unwrap(), bytes);
    limited.codex_source.as_mut().unwrap().target_unconfirmed = true;
    let provisional = native::read(&limited, None).unwrap();
    assert_eq!(
        provisional.configuration.base_url(),
        "https://after.example"
    );
    assert!(
        native_edit::write(
            &limited,
            &provisional,
            &desired,
            &[address_edit],
            &ApiKeyChange::Keep,
            None
        )
        .unwrap_err()
        .to_string()
        .contains("写入目标尚未确认")
    );
    assert_eq!(
        std::fs::read(&limited_path).unwrap(),
        bytes,
        "an existing default file is not evidence of the wrapper's write target"
    );

    // A removed legacy selector must not redirect a modern plain launch's editor.
    let mut plain = context.clone();
    plain.directory = root.join("plain-launch-legacy-content");
    private_storage::atomic_write_private_bytes(&plain.path(), b"model='root-model'\nmodel_provider='root'\nprofile='old'\n[model_providers.root]\nbase_url='https://root.example'\nexperimental_bearer_token='root-key'\n[model_providers.old]\nbase_url='https://old.example'\n[profiles.old]\nmodel='old-model'\nmodel_provider='old'\n").unwrap();
    let plain_read = native::read(&plain, None).unwrap();
    assert_eq!(
        plain_read.configuration.base_url(),
        "https://root.example",
        "only the actual launch selects a profile"
    );
    let old_bytes = std::fs::read(plain.path()).unwrap();
    plain.codex_source = Some(codex_source::Source {
        base: plain.path(),
        rejected_selector: true,
        ..Default::default()
    });
    let before = native::read(&plain, None).unwrap();
    assert_eq!(
        std::fs::read(plain.path()).unwrap(),
        old_bytes,
        "read never migrates"
    );
    let mut after = before.configuration.clone();
    if let CustomApiConfiguration::Codex { default_model, .. } = &mut after {
        *default_model = "new-root-model".into();
    }
    native_edit::write(
        &plain,
        &before,
        &after,
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
    let saved = native::read_toml(&plain.path()).unwrap();
    assert!(saved.get("profile").is_none());
    assert_eq!(saved["model"].as_str(), Some("new-root-model"));
    assert_eq!(
        saved["profiles"]["old"]["model"].as_str(),
        Some("old-model")
    );
    assert_eq!(
        saved["model_providers"]["root"]["experimental_bearer_token"].as_str(),
        Some("root-key")
    );
    // A native-reported independent User layer is merged, not inferred from inline tables.
    let base = plain.path();
    let selected = plain.directory.join("selected.config.toml");
    private_storage::atomic_write_private_bytes(
        &selected,
        b"model='profile-model'\n[model_providers.root]\nbase_url='https://profile.example'\n",
    )
    .unwrap();
    plain.codex_source = Some(codex_source::Source {
        base: base.clone(),
        profile: Some(selected.clone()),
        ..Default::default()
    });
    let before = native::read(&plain, None).unwrap();
    assert_eq!(before.configuration.base_url(), "https://profile.example");
    assert_eq!(
        before.snapshot(&plain, false).key().unwrap().as_deref(),
        Some("root-key")
    );
    let mut desired = before.configuration.clone();
    if let CustomApiConfiguration::Codex {
        base_url,
        default_model,
        ..
    } = &mut desired
    {
        *base_url = "https://edited-profile.example".into();
        *default_model = "edited-profile".into();
    }
    let edits = ["baseUrl", "defaultRowId"].map(|name| FieldEdit {
        path: vec![name.into()],
        before: Value::Null,
        after: Value::Null,
        label: String::new(),
    });
    native_edit::write(&plain, &before, &desired, &edits, &ApiKeyChange::Keep, None).unwrap();
    assert_eq!(
        native::read_toml(&base).unwrap()["model"].as_str(),
        Some("new-root-model")
    );
    let selected_doc = native::read_toml(&selected).unwrap();
    assert_eq!(selected_doc["model"].as_str(), Some("edited-profile"));
    assert_eq!(
        selected_doc["model_providers"]["root"]["base_url"].as_str(),
        Some("https://edited-profile.example")
    );
    // A relative catalog is resolved from its defining file, not Rovai's cwd.
    let mut root_doc = native::read_toml(&base).unwrap();
    root_doc["model_catalog_json"] = toml_edit::value("relative.json");
    std::fs::write(&base, root_doc.to_string()).unwrap();
    std::fs::write(
        base.parent().unwrap().join("relative.json"),
        serde_json::to_vec(&json!({"models":[]})).unwrap(),
    )
    .unwrap();
    let before = native::read(&plain, None).unwrap();
    assert_eq!(
        before.catalog_path,
        Some(base.parent().unwrap().join("relative.json"))
    );
    // Replacing an inherited credential cannot re-inherit a mutually exclusive source.
    native_edit::write(
        &plain,
        &before,
        &desired,
        &edits,
        &ApiKeyChange::Replace {
            value: "profile-key".into(),
        },
        None,
    )
    .unwrap();
    let before = native::read(&plain, None).unwrap();
    assert_eq!(
        before.snapshot(&plain, false).key().unwrap().as_deref(),
        Some("profile-key")
    );
    assert_ne!(before.provider_id, "root");
    assert_eq!(
        native::read_toml(&base).unwrap()["model_providers"]["root"]["experimental_bearer_token"]
            .as_str(),
        Some("root-key")
    );
    desired.set_mode(Some(ConnectionMode::OfficialLogin));
    native_edit::write(
        &plain,
        &before,
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
    .unwrap();
    let selected_doc = native::read_toml(&selected).unwrap();
    assert_eq!(selected_doc["model_provider"].as_str(), Some("openai"));
    assert!(selected_doc.get("model").is_none());
    assert!(
        native::read_toml(&base).unwrap().get("model").is_none(),
        "old API defaults cannot reappear by inheritance"
    );

    // Provider-owned command/AWS auth is a reusable source, without running a helper.
    for (mechanism, declaration) in [
        ("auth", "{command='/never/execute-on-read',args=['key']}"),
        ("aws", "{region='us-east-1',profile='fixture'}"),
    ] {
        let mut custom = context.clone();
        custom.directory = root.join(format!("provider-{mechanism}"));
        let text = format!(
            "model_provider='relay'\nmodel='a'\n[model_providers.relay]\nbase_url='https://before.example'\nwire_api='responses'\n{mechanism}={declaration}\nquery_params={{tag='keep'}}\nstream_idle_timeout_ms=15000\n[model_providers.other]\nexperimental_bearer_token='unrelated-key'\n"
        );
        private_storage::atomic_write_private_bytes(&custom.path(), text.as_bytes()).unwrap();
        let read = native::read(&custom, None).unwrap();
        assert_eq!(read.credential.status, "available");
        assert!(
            read.credential
                .source_label
                .contains(if mechanism == "auth" {
                    "原生命令"
                } else {
                    "AWS"
                })
        );
        assert!(
            native::credential_value(&read.source, &custom)
                .unwrap()
                .0
                .is_none()
        );
        let mut desired = read.configuration.clone();
        if let CustomApiConfiguration::Codex { base_url, .. } = &mut desired {
            *base_url = "https://after.example".into();
        }
        let edits = [FieldEdit {
            path: vec!["baseUrl".into()],
            before: json!("https://before.example"),
            after: json!("https://after.example"),
            label: String::new(),
        }];
        native_edit::write(&custom, &read, &desired, &edits, &ApiKeyChange::Keep, None).unwrap();
        let doc = native::read_toml(&custom.path()).unwrap();
        assert!(doc["model_providers"]["relay"].get(mechanism).is_some());
        let read = native::read(&custom, None).unwrap();
        native_edit::write(
            &custom,
            &read,
            &desired,
            &[],
            &ApiKeyChange::Replace {
                value: "new-static-key".into(),
            },
            None,
        )
        .unwrap();
        let doc = native::read_toml(&custom.path()).unwrap();
        let provider = &doc["model_providers"]["relay"];
        for name in ["auth", "aws", "env_key"] {
            assert!(provider.get(name).is_none());
        }
        assert_eq!(
            provider["experimental_bearer_token"].as_str(),
            Some("new-static-key")
        );
        assert_eq!(provider["query_params"]["tag"].as_str(), Some("keep"));
        assert_eq!(provider["stream_idle_timeout_ms"].as_integer(), Some(15000));
        assert_eq!(
            doc["model_providers"]["other"]["experimental_bearer_token"].as_str(),
            Some("unrelated-key")
        );
    }
    // Cloud endpoints are never fabricated as api.anthropic.com. Only an explicit
    // new Messages connection disables the selectors; model edits retain them.
    for (flag, base, route) in native::CLAUDE_CLOUD_ROUTES {
        let mut cloud = context.clone();
        cloud.kind = AdapterKind::ClaudeCodeCli;
        cloud.directory = root.join(flag);
        cloud.environment.insert(flag.into(), "1".into());
        let initial = json!({"env":{flag:"1",base:"https://cloud.example/native","ANTHROPIC_MODEL":"a","CLAUDE_CODE_OAUTH_TOKEN":"official-token","AWS_SECRET_ACCESS_KEY":"aws-private-key"},"permissions":{"defaultMode":"default"}});
        private_storage::atomic_write_private_bytes(
            &cloud.path(),
            &serde_json::to_vec(&initial).unwrap(),
        )
        .unwrap();
        let read = native::read(&cloud, None).unwrap();
        assert_eq!(
            read.configuration.base_url(),
            "https://cloud.example/native"
        );
        assert_eq!(read.credential.source, "native_cloud");
        assert_eq!(read.credential.source_label, route);
        let mut desired = read.configuration.clone();
        if let CustomApiConfiguration::ClaudeCode { models, .. } = &mut desired {
            models.model = "b".into();
        }
        native_edit::write(
            &cloud,
            &read,
            &desired,
            &[FieldEdit {
                path: vec!["claudeModels".into(), "model".into()],
                before: json!("a"),
                after: json!("b"),
                label: String::new(),
            }],
            &ApiKeyChange::Keep,
            None,
        )
        .unwrap();
        assert_eq!(native::read_json(&cloud.path()).unwrap()["env"][flag], "1");
        let read = native::read(&cloud, None).unwrap();
        if let CustomApiConfiguration::ClaudeCode { base_url, .. } = &mut desired {
            *base_url = "https://messages.example/prefix".into();
        }
        let edits = [FieldEdit {
            path: vec!["baseUrl".into()],
            before: json!("https://cloud.example/native"),
            after: json!(desired.base_url()),
            label: String::new(),
        }];
        let before = std::fs::read(cloud.path()).unwrap();
        assert!(
            native_edit::write(&cloud, &read, &desired, &edits, &ApiKeyChange::Keep, None).is_err()
        );
        assert_eq!(std::fs::read(cloud.path()).unwrap(), before);
        let mut host_managed = cloud.clone();
        host_managed
            .environment
            .insert("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST".into(), "1".into());
        let error = native_edit::write(
            &host_managed,
            &read,
            &desired,
            &edits,
            &ApiKeyChange::Replace {
                value: "messages-key".into(),
            },
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("外部宿主管理"));
        assert_eq!(std::fs::read(cloud.path()).unwrap(), before);
        native_edit::write(
            &cloud,
            &read,
            &desired,
            &edits,
            &ApiKeyChange::Replace {
                value: "messages-key".into(),
            },
            None,
        )
        .unwrap();
        let doc = native::read_json(&cloud.path()).unwrap();
        assert_eq!(
            doc["env"][flag], "0",
            "saved native mask also disables inherited selection"
        );
        assert_eq!(
            doc["env"][base], "https://cloud.example/native",
            "inactive cloud fields are retained"
        );
        assert_eq!(doc["env"]["CLAUDE_CODE_OAUTH_TOKEN"], "official-token");
        assert_eq!(doc["env"]["AWS_SECRET_ACCESS_KEY"], "aws-private-key");
        let read = native::read(&cloud, None).unwrap();
        assert_eq!(
            read.configuration.base_url(),
            "https://messages.example/prefix"
        );
        assert_ne!(read.credential.source, "native_cloud");
    }
    for store in ["file", "auto"] {
        let mut api_only = context.clone();
        api_only.directory = root.join(format!("api-only-{store}"));
        private_storage::atomic_write_private_bytes(
            &api_only.path(),
            format!("cli_auth_credentials_store='{store}'\n").as_bytes(),
        )
        .unwrap();
        let auth_path = api_only.directory.join("auth.json");
        std::fs::write(
            &auth_path,
            br#"{"auth_mode":"apikey","OPENAI_API_KEY":"fake-key","unknown":true}"#,
        )
        .unwrap();
        let read = native::read(&api_only, None).unwrap();
        let mut official = read.configuration.clone();
        official.set_mode(Some(ConnectionMode::OfficialLogin));
        native_edit::write(
            &api_only,
            &read,
            &official,
            &[FieldEdit {
                path: vec!["mode".into()],
                before: Value::Null,
                after: json!("official_login"),
                label: String::new(),
            }],
            &ApiKeyChange::Keep,
            None,
        )
        .unwrap();
        let auth = native::read_json(&auth_path).unwrap();
        assert!(auth.get("OPENAI_API_KEY").is_none());
        assert_eq!(
            auth["auth_mode"], "apikey",
            "do not manufacture a ChatGPT identity from an empty auth object"
        );
        assert_eq!(auth["unknown"], true);
        assert_eq!(
            native::read_toml(&api_only.path()).unwrap()["forced_login_method"].as_str(),
            Some("chatgpt")
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let wrapper = root.join("account-cli");
        std::fs::write(&wrapper, r##"#!/usr/bin/env python3
import json,os,pathlib,re,sys
root=pathlib.Path(os.environ['CODEX_HOME'])
for line in sys.stdin:
 q=json.loads(line)
 with (root/'calls').open('a') as f: f.write(json.dumps(q)+'\n')
 if q['method']=='initialize': print(json.dumps({'id':q['id'],'result':{'userAgent':'fixture'}}),flush=True)
 if q['method']=='account/read':
  a=json.loads((root/'account.json').read_text())
  if re.search(r'forced_login_method\s*=\s*[\"\x27]chatgpt', (root/'config.toml').read_text()) and a.get('type')=='apiKey': a=None
  print(json.dumps({'id':q['id'],'result':{'account':a,'requiresOpenaiAuth':True}}),flush=True)
"##).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let source_wrapper = root.join("source-wrapper");
        std::fs::write(&source_wrapper, r##"#!/usr/bin/env python3
import json,os,pathlib,sys
if '--version' in sys.argv: print('codex-cli 0.159.2'); sys.exit(0)
root=pathlib.Path(os.environ['CODEX_HOME'])/'actual'
for line in sys.stdin:
 q=json.loads(line)
 if q['method']=='initialize': print(json.dumps({'id':q['id'],'result':{}}),flush=True)
 if q['method']=='config/read': print(json.dumps({'id':q['id'],'result':{'config':{'model_provider':'relay'},'origins':{'model':{'name':{'type':'sessionFlags'}},'model_provider':{'name':{'type':'sessionFlags'}}},'layers':[{'name':{'type':'user','file':str(root/'config.toml'),'profile':None}}]}}),flush=True)
"##).unwrap();
        std::fs::set_permissions(&source_wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut via_wrapper = context.clone();
        via_wrapper.directory = root.join("source-wrapper-home");
        via_wrapper.environment = BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]);
        private_storage::atomic_write_private_bytes(&via_wrapper.directory.join("actual/config.toml"), b"model='actual-model'\nmodel_provider='relay'\n[model_providers.relay]\nbase_url='https://actual.example'\nexperimental_bearer_token='actual-fake-key'\n").unwrap();
        codex_source::refresh(&source_wrapper, &via_wrapper, &via_wrapper).await;
        via_wrapper.codex_source = codex_source::resolve(&via_wrapper);
        let found = native::read(&via_wrapper, None).unwrap();
        assert_eq!(found.configuration.base_url(), "https://actual.example");
        assert!(
            found
                .observation
                .conflict
                .as_deref()
                .unwrap()
                .contains("默认模型")
        );
        assert_eq!(
            via_wrapper.path(),
            via_wrapper.directory.join("actual/config.toml")
        );
        let mut desired = found.configuration.clone();
        if let CustomApiConfiguration::Codex { base_url, .. } = &mut desired {
            *base_url = "https://actual-edited.example".into();
        }
        native_edit::write(
            &via_wrapper,
            &found,
            &desired,
            &[FieldEdit {
                path: vec!["baseUrl".into()],
                before: Value::Null,
                after: Value::Null,
                label: String::new(),
            }],
            &ApiKeyChange::Keep,
            None,
        )
        .unwrap();
        assert_eq!(
            native::read(&via_wrapper, None)
                .unwrap()
                .configuration
                .base_url(),
            "https://actual-edited.example"
        );
        assert!(
            !via_wrapper.directory.join("config.toml").exists(),
            "a wrapper's default directory must not become a second source"
        );
        for store in ["keyring", "auto"] {
            let mut opaque = context.clone();
            opaque.directory = root.join(format!("account-{store}"));
            opaque.environment = BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]);
            private_storage::atomic_write_private_bytes(
                &opaque.path(),
                format!("cli_auth_credentials_store='{store}'\nmodel='native-api-default'\n")
                    .as_bytes(),
            )
            .unwrap();
            let account = opaque.directory.join("account.json");
            std::fs::write(&account, br#"{"type":"apiKey"}"#).unwrap();
            if store == "keyring" {
                std::fs::write(
                    opaque.directory.join("auth.json"),
                    b"unrelated-invalid-file",
                )
                .unwrap();
            }
            let before = std::fs::read(opaque.path()).unwrap();
            let read = native::read(&opaque, None).unwrap();
            let identity = read.snapshot(&opaque, false).identity().unwrap();
            assert_eq!(
                read.configuration.mode(),
                None,
                "a managed store is not proof of ChatGPT"
            );
            codex_native::refresh(&wrapper, &opaque).await;
            let mut projected = native::read(&opaque, None).unwrap();
            codex_native::project(
                &opaque,
                &mut projected.configuration,
                &mut projected.credential,
                &mut projected.observation,
            );
            assert_eq!(
                projected.observation.initial_mode,
                Some(ConnectionMode::CustomApi)
            );
            assert_eq!(
                projected.configuration.mode(),
                None,
                "identity cannot rewrite the file baseline"
            );
            assert_eq!(projected.credential.status, "available");
            assert_eq!(std::fs::read(opaque.path()).unwrap(), before);
            assert_eq!(
                native::read(&opaque, None)
                    .unwrap()
                    .snapshot(&opaque, false)
                    .identity()
                    .unwrap(),
                identity,
                "display observation does not change execution binding"
            );
            let mut official = projected.configuration.clone();
            official.set_mode(Some(ConnectionMode::OfficialLogin));
            native_edit::write(
                &opaque,
                &read,
                &official,
                &[FieldEdit {
                    path: vec!["mode".into()],
                    before: json!("custom_api"),
                    after: json!("official_login"),
                    label: String::new(),
                }],
                &ApiKeyChange::Keep,
                None,
            )
            .unwrap();
            assert_eq!(
                native::read_toml(&opaque.path()).unwrap()["forced_login_method"].as_str(),
                Some("chatgpt")
            );
            assert!(
                native::read_toml(&opaque.path())
                    .unwrap()
                    .get("model")
                    .is_none()
            );
            assert_eq!(
                std::fs::read_to_string(&account).unwrap(),
                r#"{"type":"apiKey"}"#,
                "Rovai does not delete a native keyring object"
            );
            codex_native::refresh(&wrapper, &opaque).await;
            let mut projected = native::read(&opaque, None).unwrap();
            codex_native::project(
                &opaque,
                &mut projected.configuration,
                &mut projected.credential,
                &mut projected.observation,
            );
            assert_eq!(
                projected.configuration.mode(),
                Some(ConnectionMode::OfficialLogin)
            );
            assert_eq!(projected.observation.login_status, "signed_out");
            std::fs::write(&account, br#"{"type":"chatgpt"}"#).unwrap();
            codex_native::refresh(&wrapper, &opaque).await;
            let mut projected = native::read(&opaque, None).unwrap();
            codex_native::project(
                &opaque,
                &mut projected.configuration,
                &mut projected.credential,
                &mut projected.observation,
            );
            assert_eq!(projected.observation.login_status, "signed_in");
            codex_native::refresh(&root.join("missing-native-cli"), &opaque).await;
            assert_eq!(
                codex_native::observed(&opaque),
                codex_native::Identity::Unknown
            );
            let calls = std::fs::read_to_string(opaque.directory.join("calls")).unwrap();
            for line in calls.lines() {
                let q: Value = serde_json::from_str(line).unwrap();
                assert!(matches!(
                    q["method"].as_str(),
                    Some("initialize" | "initialized" | "account/read")
                ));
                if q["method"] == "account/read" {
                    assert_eq!(q["params"]["refreshToken"], false);
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
