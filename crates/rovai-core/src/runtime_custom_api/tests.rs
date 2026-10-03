use super::*;
use crate::runtime_startup::{self, RuntimeEnvironmentVariable, RuntimeStartupConfiguration};

fn configuration(kind: AdapterKind) -> CustomApiConfiguration {
    let base_url = "https://relay.example/prefix".into();
    match kind {
        AdapterKind::ClaudeCodeCli => CustomApiConfiguration::ClaudeCode { enabled: true, base_url,
            models: ClaudeApiModels { model: "main".into(), reasoning_model: "think".into(), haiku_model: "small".into(), sonnet_model: "medium".into(), opus_model: "large".into() } },
        AdapterKind::CodexCli => CustomApiConfiguration::Codex { enabled: true, base_url,
            models: vec![CustomApiModel { id: "model-a".into(), display_name: "Development".into() }, CustomApiModel { id: "model-b".into(), display_name: String::new() }], default_model: "model-a".into() },
        AdapterKind::KimiCodeCli => CustomApiConfiguration::KimiCode { enabled: true, base_url, api_type: KimiApiType::Openai, model: "private-model".into() },
        AdapterKind::GrokBuild => CustomApiConfiguration::GrokBuild { enabled: true, base_url, model: "grok-4.6".into() },
        _ => unreachable!(),
    }
}

// Owns the new closed input boundary; existing generic-env validation has no structured API fields.
#[test]
fn configuration_rejects_ambiguous_connections_and_preserves_optional_models() {
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli, AdapterKind::KimiCodeCli, AdapterKind::GrokBuild] {
        let mut config = configuration(kind);
        config.validate(kind).unwrap();
        assert_eq!(config.base_url(), "https://relay.example/prefix");
        assert!(config.validate(AdapterKind::Pi).is_err());
        let forbidden = match kind { AdapterKind::ClaudeCodeCli => "ANTHROPIC_AUTH_TOKEN", AdapterKind::CodexCli => "OPENAI_BASE_URL", AdapterKind::KimiCodeCli => "KIMI_MODEL_API_KEY", _ => "XAI_API_KEY" };
        assert!(RuntimeStartupConfiguration { custom_api: Some(config), environment: vec![RuntimeEnvironmentVariable { name: forbidden.into(), value: "unchanged".into() }], ..Default::default() }.validated(false).is_err());
    }
    for url in ["https://name:password@relay.example/prefix", "ftp://relay.example", "https://relay.example/#fragment", ""] {
        let mut config = CustomApiConfiguration::GrokBuild { enabled: true, base_url: url.into(), model: "grok-4.6".into() };
        assert!(config.validate(AdapterKind::GrokBuild).is_err());
    }
    let mut claude = CustomApiConfiguration::ClaudeCode { enabled: true, base_url: "http://127.0.0.1/prefix".into(), models: ClaudeApiModels::default() };
    claude.validate(AdapterKind::ClaudeCodeCli).unwrap();
    let mut codex = configuration(AdapterKind::CodexCli);
    if let CustomApiConfiguration::Codex { models, default_model, .. } = &mut codex { models.remove(0); *default_model = "model-a".into(); }
    assert!(codex.validate(AdapterKind::CodexCli).is_err());
    if let CustomApiConfiguration::Codex { default_model, .. } = &mut codex { *default_model = "model-b".into(); }
    codex.validate(AdapterKind::CodexCli).unwrap();
    if let CustomApiConfiguration::Codex { models, .. } = &mut codex { models.push(models[0].clone()); }
    assert!(codex.validate(AdapterKind::CodexCli).is_err());
    let mut disabled = CustomApiConfiguration::Codex { enabled: false, base_url: String::new(), models: vec![], default_model: String::new() };
    disabled.validate(AdapterKind::CodexCli).unwrap();
    assert!(!disabled.conflicts_with_environment("OPENAI_BASE_URL"));
    assert!(serde_json::from_value::<CustomApiConfiguration>(json!({"kind":"grok-build","enabled":false,"baseUrl":"","model":"","apiKey":"never-store-me"})).is_err());
    assert!(serde_json::from_value::<RuntimeStartupConfiguration>(json!({"programPath":null,"environment":[],"customApiSnapshot":{}})).is_err());
}

#[cfg(feature = "extended-tests")]
#[test]
fn private_credentials_are_versioned_and_retained_only_for_saved_or_frozen_configuration() {
    let mut db = crate::test_support::seeded_runtime_database_owned();
    // The GC query owns status + frozen JSON, not Run creation. A local table avoids unrelated scheduling fixtures.
    db.connection().execute_batch("CREATE TEMP TABLE agent_run (status TEXT, effective_config_json TEXT)").unwrap();
    let kind = AdapterKind::ClaudeCodeCli;
    let config = RuntimeStartupConfiguration { custom_api: Some(configuration(kind)), ..Default::default() };
    assert!(!runtime_startup::load(&db, kind).unwrap().api_key_configured);
    assert!(runtime_startup::save_with_api_key(&mut db, kind, 0, config.clone(), 1, ApiKeyChange::Keep).is_err());
    let first = runtime_startup::save_with_api_key(&mut db, kind, 0, config.clone(), 1, ApiKeyChange::Replace { value: "isolated-key-one".into() }).unwrap();
    let old = first.configuration.custom_api_snapshot.as_ref().unwrap().clone();
    let public = serde_json::to_string(&first).unwrap();
    let stored: String = db.connection().query_row("SELECT configuration_json FROM runtime_startup_setting", [], |row| row.get(0)).unwrap();
    for value in [&public, &stored] { assert!(!value.contains("isolated-key-one")); }
    assert!(public.contains("apiKeyConfigured"));
    assert!(!public.contains("credentialVersion"));
    let artifact = old.write_artifact("settings.json", b"old immutable settings").unwrap();
    assert!(old.write_artifact("settings.json", b"new settings").is_err());
    assert_eq!(std::fs::read(&artifact).unwrap(), b"old immutable settings");
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(credential_path(&old.storage_root, &old.credential_version).unwrap()).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&old.storage_root).unwrap().permissions().mode() & 0o777, 0o700);
    }
    db.connection().execute("INSERT INTO agent_run VALUES('running', ?1)", [json!({"runtime":{"customApi":old}}).to_string()]).unwrap();
    let (draft, guard) = runtime_startup::resolve_draft(&db, kind, config.clone(), ApiKeyChange::Keep).unwrap();
    let draft_snapshot = draft.custom_api_snapshot.unwrap();
    let second = runtime_startup::save_with_api_key(&mut db, kind, 1, config.clone(), 2, ApiKeyChange::Replace { value: "isolated-key-two".into() }).unwrap();
    let current = second.configuration.custom_api_snapshot.clone().unwrap();
    assert_ne!(old.identity().unwrap(), current.identity().unwrap());
    assert_eq!(old.key().unwrap(), "isolated-key-one");
    assert_eq!(current.key().unwrap(), "isolated-key-two");
    assert_eq!(draft_snapshot.key().unwrap(), "isolated-key-one");
    assert!(runtime_startup::save_with_api_key(&mut db, kind, 1, config.clone(), 3, ApiKeyChange::Replace { value: "stale-key".into() }).is_err());
    assert_eq!(runtime_startup::load(&db, kind).unwrap().revision, 2);
    let mut cleared_models = config.clone();
    if let Some(CustomApiConfiguration::ClaudeCode { models, .. }) = &mut cleared_models.custom_api { *models = ClaudeApiModels::default(); }
    let third = runtime_startup::save_with_api_key(&mut db, kind, 2, cleared_models.clone(), 3, ApiKeyChange::Keep).unwrap();
    let environment = third.configuration.custom_api_snapshot.unwrap().environment().unwrap();
    assert_eq!(environment.len(), 2, "cleared model fields must stop injecting values");
    assert_eq!(environment["ANTHROPIC_AUTH_TOKEN"], "isolated-key-two");
    assert!(runtime_startup::save_with_api_key(&mut db, kind, 3, cleared_models.clone(), 4, ApiKeyChange::Clear).is_err());
    if let Some(CustomApiConfiguration::ClaudeCode { enabled, .. }) = &mut cleared_models.custom_api { *enabled = false; }
    let fourth = runtime_startup::save_with_api_key(&mut db, kind, 3, cleared_models.clone(), 4, ApiKeyChange::Keep).unwrap();
    assert!(fourth.api_key_configured && fourth.configuration.custom_api_snapshot.is_none());
    let fifth = runtime_startup::save_with_api_key(&mut db, kind, 4, cleared_models, 5, ApiKeyChange::Clear).unwrap();
    assert!(!fifth.api_key_configured);
    assert!(current.key().is_err());
    assert_eq!(old.key().unwrap(), "isolated-key-one", "running snapshot keeps its old key");
    db.connection().execute("UPDATE agent_run SET status='succeeded'", []).unwrap();
    runtime_startup::cleanup_unused_credentials(&db).unwrap();
    assert!(old.key().is_err());
    assert!(!artifact.exists());
    assert_eq!(draft_snapshot.key().unwrap(), "isolated-key-one", "private preview lifetime is independent of saved key GC");
    drop(guard);
    assert!(draft_snapshot.key().is_err());
}

#[cfg(feature = "extended-tests")]
#[test]
fn four_native_environments_and_private_diagnostics_use_the_same_connection() {
    let db = crate::test_support::seeded_runtime_database_owned();
    let key = "local-fixture-secret";
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli, AdapterKind::KimiCodeCli, AdapterKind::GrokBuild] {
        let root = storage_root(db.path(), kind).unwrap();
        let snapshot = CustomApiSnapshot { configuration: configuration(kind), revision: 1, credential_version: write_credential(&root, key).unwrap(), storage_root: root };
        let env = snapshot.environment().unwrap();
        let key_name = match kind { AdapterKind::ClaudeCodeCli => "ANTHROPIC_AUTH_TOKEN", AdapterKind::CodexCli => "ROVAI_CUSTOM_API_KEY", AdapterKind::KimiCodeCli => "KIMI_MODEL_API_KEY", _ => "XAI_API_KEY" };
        assert_eq!(env[key_name], key);
        if kind == AdapterKind::ClaudeCodeCli {
            assert_eq!(env["ANTHROPIC_REASONING_MODEL"], "think");
            assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "small");
            assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "medium");
            assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "large");
        }
        if kind == AdapterKind::KimiCodeCli {
            let mut native = json!({"configOptions":[{"id":"model","currentValue":"__kimi_env_model__","options":[{"value":"old-provider","name":"old"},{"value":"__kimi_env_model__","name":"temporary"}]}]});
            project_kimi_session(&snapshot, &mut native).unwrap();
            assert_eq!(native["configOptions"][0]["currentValue"], "private-model");
            assert_eq!(native["configOptions"][0]["options"].as_array().unwrap().len(), 1);
            assert!(project_kimi_session(&snapshot, &mut native).is_err(), "a different explicit provider must be rejected");
        }
        let mut output = json!({"error":format!("rejected {key}"),"result":[key,{key:key}]});
        snapshot.redactor().unwrap().value(&mut output);
        assert!(!output.to_string().contains(key));
        assert!(!serde_json::to_string(&snapshot).unwrap().contains(key));
    }
    for bad in ["", "***", "has space", "line\nkey"] { assert!(write_credential(db.directory(), bad).is_err()); }
}
