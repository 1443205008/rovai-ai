//! Integration owner: live Check Manager + private database + synthetic programs.
//! The capture seam never reads a developer's PATH, registry, shell or accounts.
use super::*;
use crate::application::{RuntimeCheckOutcome, RuntimeCheckTrigger, RuntimeLaunchPurpose};
use rovai_core::runtime_startup::{RuntimeEnvironmentVariable, RuntimeStartupConfiguration};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

const KIND: AdapterKind = AdapterKind::CodexCli;

struct Fixture {
    root: PathBuf,
    core: Arc<Core>,
    paths: Arc<StdMutex<Vec<PathBuf>>>,
    captures: Arc<AtomicUsize>,
    shutdown: Option<oneshot::Sender<()>>,
    manager: Option<tokio::task::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_capture(None)
    }

    fn with_capture(capture: Option<TestSearchCapture>) -> Self {
        let root =
            std::env::temp_dir().join(format!("rovai-check-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let root =
            rovai_core::runtime_discovery::runtime_visible_path(root.canonicalize().unwrap());
        let paths = Arc::new(StdMutex::new(Vec::new()));
        let captures = Arc::new(AtomicUsize::new(0));
        let mut core = crate::application::tests::runtime_resolution_test_core(&root).unwrap();
        core.runtime_search_capture = capture.or_else(|| {
            Some({
                let paths = paths.clone();
                let captures = captures.clone();
                Arc::new(move |generation, interactive| {
                    assert!(
                        interactive,
                        "user checks must read the interactive search source"
                    );
                    captures.fetch_add(1, Ordering::SeqCst);
                    RuntimeSearchEnvironment::for_test_paths(
                        generation,
                        paths.lock().unwrap().clone(),
                    )
                })
            })
        });
        let (requests, receiver) = mpsc::unbounded_channel();
        core.runtime_check_requests = requests;
        let core = Arc::new(core);
        let (shutdown, stopped) = oneshot::channel();
        let manager = tokio::spawn(crate::application::process_runtime_check_manager(
            core.clone(),
            receiver,
            stopped,
        ));
        Self {
            root,
            core,
            paths,
            captures,
            shutdown: Some(shutdown),
            manager: Some(manager),
        }
    }

    async fn close(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        self.manager.take().unwrap().await.unwrap();
        // Drop all private owners before removing only this fixture's unique directory.
        drop(self.core);
        std::fs::remove_dir_all(self.root).unwrap();
    }

    fn program(&self, directory: &str, version: &str, gate: bool) -> PathBuf {
        let directory = self.root.join(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(if cfg!(windows) { "codex.cmd" } else { "codex" });
        let marker = directory.join("environment.txt");
        let started = self.root.join("probe-started");
        let released = self.root.join("probe-release");
        #[cfg(windows)]
        let body = format!(
            "@echo off\r\necho %CHECK_REFRESH_VALUE%>\"{}\"\r\nif \"%~1\"==\"--version\" (\r\necho codex-cli {}\r\nexit /b 0\r\n)\r\n{}exit /b 1\r\n",
            marker.display(),
            version,
            if gate {
                format!(
                    "echo started>\"{}\"\r\n:wait\r\nif not exist \"{}\" goto wait\r\n",
                    started.display(),
                    released.display()
                )
            } else {
                String::new()
            },
        );
        #[cfg(not(windows))]
        let body = format!(
            "#!/bin/sh\nprintf '%s' \"${{CHECK_REFRESH_VALUE-unset}}\" > '{}'\nif [ \"$1\" = '--version' ]; then printf 'codex-cli {}\\n'; exit 0; fi\n{}exit 1\n",
            marker.display(),
            version,
            if gate {
                format!(
                    "printf started > '{}'\nwhile [ ! -f '{}' ]; do /bin/sleep 0.01; done\n",
                    started.display(),
                    released.display()
                )
            } else {
                String::new()
            },
        );
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }

    async fn check(&self) -> RuntimeCheckOutcome {
        check(self.core.clone()).await
    }

    async fn settings(&self) -> Value {
        self.core
            .handle_runtime_startup("runtime.startup.get", json!({"runtimeKind": KIND}))
            .await
            .unwrap()
    }

    async fn save(&self, revision: u64, configuration: &RuntimeStartupConfiguration) {
        self.core.handle_runtime_startup("runtime.startup.save", json!({
            "runtimeKind": KIND, "expectedRevision": revision, "configuration": configuration,
        })).await.unwrap();
    }

    async fn public_state(&self) -> Value {
        let mut payload = self.core.runtime_health_payload().await.unwrap();
        // Unobserved runtimes synthesize a fresh detecting timestamp on every
        // read. Keep every status/diagnostic/selection, excluding only that clock.
        for entry in payload["runtimeAvailability"].as_array_mut().unwrap() {
            if entry["discovery"]["searchGeneration"] == 0 {
                entry["discovery"]
                    .as_object_mut()
                    .unwrap()
                    .remove("observedAt");
            }
        }
        payload
    }

    async fn health(&self) -> Value {
        let payload = self.core.runtime_health_payload().await.unwrap();
        payload["runtimeAvailability"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["runtimeKind"] == "codex-cli")
            .unwrap()
            .clone()
    }
}

async fn check(core: Arc<Core>) -> RuntimeCheckOutcome {
    tokio::time::timeout(
        Duration::from_secs(15),
        core.await_runtime_check(
            KIND,
            RuntimeLaunchPurpose::AvailabilityCheck,
            RuntimeCheckTrigger::UserCheck,
        ),
    )
    .await
    .unwrap()
    .unwrap()
}

fn configuration(
    path: Option<&Path>,
    value: &str,
    process_path: &Path,
) -> RuntimeStartupConfiguration {
    RuntimeStartupConfiguration {
        custom_api: None,
        custom_api_snapshot: None,
        program_path: path.map(|path| path.to_string_lossy().to_string()),
        environment: vec![
            RuntimeEnvironmentVariable {
                name: "CHECK_REFRESH_VALUE".into(),
                value: value.into(),
            },
            RuntimeEnvironmentVariable {
                name: "PATH".into(),
                value: process_path.to_string_lossy().to_string(),
            },
        ],
    }
}

// A real manager/database seam is needed: a pure discovery test cannot prove
// which version is publicly returned after a failed probe or draft isolation.
#[tokio::test]
async fn fresh_formal_and_draft_checks_preserve_program_selection_and_private_state() {
    let fixture = Fixture::new();
    let old = fixture.program("old", "1.0.0", false);
    let new = fixture.program("new", "2.0.0", false);
    *fixture.paths.lock().unwrap() = vec![old.parent().unwrap().into()];
    fixture.settings().await;
    fixture.health().await;
    assert_eq!(
        fixture.captures.load(Ordering::SeqCst),
        0,
        "status reads do not check programs"
    );
    assert_eq!(fixture.check().await, RuntimeCheckOutcome::StableFailure);
    assert_eq!(fixture.health().await["reportedVersion"], "codex-cli 1.0.0");
    *fixture.paths.lock().unwrap() = vec![new.parent().unwrap().into()];
    assert_eq!(fixture.check().await, RuntimeCheckOutcome::StableFailure);
    assert_eq!(fixture.health().await["reportedVersion"], "codex-cli 2.0.0");
    assert_ne!(
        fixture.health().await["status"],
        "ready",
        "version alone is not a successful check"
    );
    fixture.program("new", "3.0.0", false);
    fixture.check().await;
    assert_eq!(fixture.health().await["reportedVersion"], "codex-cli 3.0.0");

    let saved = configuration(Some(&old), "saved-private", new.parent().unwrap());
    fixture.save(0, &saved).await;
    fixture.check().await;
    assert_eq!(
        fixture.health().await["reportedVersion"],
        "codex-cli 1.0.0",
        "process PATH does not select the main executable"
    );
    assert_eq!(
        std::fs::read_to_string(old.parent().unwrap().join("environment.txt"))
            .unwrap()
            .trim(),
        "saved-private"
    );
    let public_before = fixture.public_state().await;
    let settings_before = fixture.settings().await;
    let draft = configuration(Some(&new), "draft-private", old.parent().unwrap());
    let preview = fixture
        .core
        .handle_runtime_startup(
            "runtime.startup.check",
            json!({
                "runtimeKind": KIND, "configuration": draft,
            }),
        )
        .await
        .unwrap();
    assert_eq!(preview["reportedVersion"], "codex-cli 3.0.0");
    assert_eq!(
        std::fs::read_to_string(new.parent().unwrap().join("environment.txt"))
            .unwrap()
            .trim(),
        "draft-private"
    );
    assert_eq!(fixture.settings().await, settings_before);
    assert_eq!(fixture.public_state().await, public_before);
    // Replace the program between its version and authentication probes. A
    // private preview must reject the mixed identity without touching public state.
    fixture.program("new", "3.1.0", true);
    let draft_core = fixture.core.clone();
    let changed_draft = configuration(Some(&new), "draft-private", old.parent().unwrap());
    let changing_preview = tokio::spawn(async move {
        draft_core
            .handle_runtime_startup(
                "runtime.startup.check",
                json!({
                    "runtimeKind": KIND, "configuration": changed_draft,
                }),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !fixture.root.join("probe-started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fixture.program("new", "3.2.0", false);
    std::fs::write(fixture.root.join("probe-release"), b"release").unwrap();
    assert!(changing_preview.await.unwrap().is_err());
    assert_eq!(fixture.public_state().await, public_before);
    std::fs::rename(&old, old.with_extension("moved")).unwrap();
    fixture.check().await;
    let missing = fixture.health().await;
    assert_eq!(missing["discovery"]["discoveryStatus"], "missing");
    assert!(
        missing["reportedVersion"].is_null(),
        "never label a missing manual program with an old version"
    );
    assert_eq!(fixture.settings().await, settings_before);

    let other = configuration(None, "other-private", new.parent().unwrap());
    fixture
        .core
        .handle_runtime_startup(
            "runtime.startup.save",
            json!({"runtimeKind": "pi", "expectedRevision": 0, "configuration": other}),
        )
        .await
        .unwrap();
    // Restore-auto may be explicitly inspected, but saving only records that
    // selection. It must not publish the draft's fresh discovery.
    let latest = fixture.program("latest", "4.0.0", false);
    *fixture.paths.lock().unwrap() = vec![latest.parent().unwrap().into()];
    let public_before = fixture.public_state().await;
    let automatic = configuration(None, "draft-auto", old.parent().unwrap());
    let preview = fixture
        .core
        .handle_runtime_startup(
            "runtime.startup.inspect",
            json!({
                "runtimeKind": KIND, "configuration": automatic,
            }),
        )
        .await
        .unwrap();
    assert_eq!(preview["reportedVersion"], "codex-cli 4.0.0");
    assert_eq!(
        fixture.settings().await,
        settings_before,
        "restore-auto is still only a draft"
    );
    assert_eq!(fixture.public_state().await, public_before);
    assert!(!public_before.to_string().contains("saved-private"));
    assert!(!preview.to_string().contains("draft-auto"));
    assert!(fixture.captures.load(Ordering::SeqCst) >= 6);

    assert!(
        fixture
            .core
            .handle_runtime_startup(
                "runtime.startup.save",
                json!({
                    "runtimeKind": KIND, "expectedRevision": 0, "configuration": automatic,
                })
            )
            .await
            .is_err(),
        "a stale save cannot publish the fresh search environment"
    );
    assert_eq!(fixture.settings().await, settings_before);
    assert_eq!(fixture.public_state().await, public_before);
    let captures_before_save = fixture.captures.load(Ordering::SeqCst);
    let discovery_before_save = public_before["runtimeAvailability"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["runtimeKind"] == KIND.as_str())
        .unwrap()["discovery"]
        .clone();
    fixture.save(1, &automatic).await;
    assert_eq!(
        fixture.captures.load(Ordering::SeqCst),
        captures_before_save
    );
    assert_eq!(
        fixture.health().await["discovery"],
        discovery_before_save,
        "save cannot trigger or publish discovery"
    );
    assert_eq!(
        fixture
            .core
            .runtime_search_environment
            .read()
            .await
            .startup_configuration(AdapterKind::Pi),
        other,
        "refreshing the base environment preserves other saved Runtime settings"
    );
    let captures = fixture.captures.load(Ordering::SeqCst);
    fixture.save(1, &automatic).await;
    assert_eq!(
        fixture.captures.load(Ordering::SeqCst),
        captures,
        "same-write retries remain idempotent"
    );
    // Write-only API-key input never appears in the RPC's successful readback or errors.
    let api_kind = AdapterKind::ClaudeCodeCli;
    let current = fixture
        .core
        .handle_runtime_startup("runtime.startup.get", json!({"runtimeKind":api_kind}))
        .await
        .unwrap();
    let saved = fixture.core.handle_runtime_startup("runtime.startup.save", json!({
        "runtimeKind":api_kind,"edits":[
            {"path":["mode"],"before":current["configuration"]["customApi"]["mode"],"after":"custom_api","label":"连接方式"},
            {"path":["baseUrl"],"before":current["configuration"]["customApi"]["baseUrl"],"after":"https://offline.invalid/prefix","label":"地址"},
            {"path":["credentialVersion"],"before":current["credential"]["version"],"after":"replace","label":"API Key"}
        ],"apiKey":{"action":"replace","value":"private-rpc-test-key"}
    })).await.unwrap();
    assert_eq!(saved["credential"]["status"], "available");
    assert!(!saved.to_string().contains("private-rpc-test-key"));
    let read = fixture
        .core
        .handle_runtime_startup("runtime.startup.get", json!({"runtimeKind":api_kind}))
        .await
        .unwrap();
    assert_eq!(read["configuration"], saved["configuration"]);
    assert!(
        fixture
            .core
            .runtime_search_environment
            .read()
            .await
            .startup_configuration(api_kind)
            .custom_api_snapshot
            .is_some()
    );
    let bad = fixture
        .core
        .handle_runtime_startup(
            "runtime.startup.save",
            json!({
                "runtimeKind":api_kind,"edits":[],"apiKey":{"action":"private-rpc-test-key"}
            }),
        )
        .await
        .unwrap_err();
    assert!(!format!("{bad:#}").contains("private-rpc-test-key"));
    // A stalled optional native process cannot own the settings page's response.
    let stalled = fixture.program("settings-only", "0.159.2", true);
    let native_home = fixture.root.join("settings-native");
    std::fs::create_dir_all(&native_home).unwrap();
    std::fs::write(native_home.join("config.toml"), "model_provider='relay'\nmodel='local-model'\n[model_providers.relay]\nbase_url='https://local.example'\nexperimental_bearer_token='local-fake-key'\n").unwrap();
    let mut startup = configuration(Some(&stalled), "local", stalled.parent().unwrap());
    startup.environment.push(RuntimeEnvironmentVariable {
        name: "CODEX_HOME".into(),
        value: native_home.to_string_lossy().into_owned(),
    });
    {
        let mut db = fixture.core.database.lock().await;
        let before = rovai_core::runtime_startup::load(&db, KIND).unwrap();
        rovai_core::runtime_startup::save(&mut db, KIND, before.revision, startup, 100).unwrap();
    }
    let local = tokio::time::timeout(Duration::from_secs(2), fixture.settings())
        .await
        .expect("local settings must not await the stalled auxiliary child");
    assert_eq!(
        local["configuration"]["customApi"]["baseUrl"],
        "https://local.example"
    );
    assert!(
        !stalled.parent().unwrap().join("environment.txt").exists(),
        "get must not launch a native process"
    );
    let observer = {
        let core = fixture.core.clone();
        tokio::spawn(async move {
            core.handle_runtime_startup("runtime.startup.observe", json!({"runtimeKind":KIND}))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.root.join("probe-started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // The auxiliary child is held at a file barrier while both operations finish.
    let local = tokio::time::timeout(Duration::from_secs(2), fixture.settings())
        .await
        .unwrap();
    assert_eq!(
        local["configuration"]["customApi"]["baseUrl"],
        "https://local.example"
    );
    let saved = tokio::time::timeout(Duration::from_secs(2), fixture.core.handle_runtime_startup("runtime.startup.save", json!({
        "runtimeKind":KIND,"edits":[{"path":["environment","VISIBLE_SETTING"],"before":null,"after":"on","label":""}]
    }))).await.unwrap().unwrap();
    assert!(
        saved["configuration"]["environment"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "VISIBLE_SETTING")
    );
    let original = std::fs::read(native_home.join("config.toml")).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(2), fixture.core.handle_runtime_startup("runtime.startup.save", json!({
        "runtimeKind":KIND,"edits":[{"path":["baseUrl"],"before":"https://local.example","after":"https://never-write.example","label":""}]
    }))).await.unwrap().unwrap_err();
    assert!(error.to_string().contains("写入目标尚未确认"));
    assert_eq!(
        std::fs::read(native_home.join("config.toml")).unwrap(),
        original
    );
    std::fs::write(fixture.root.join("probe-release"), "release").unwrap();
    let observed = observer.await.unwrap().unwrap();
    assert!(
        observed["connectionReadError"].is_null(),
        "failed supplement never clears the local form"
    );
    fixture.close().await;
}

// Controlled child/file barriers own the interleaving, not scheduler timing.
#[tokio::test]
async fn saved_configuration_and_new_check_cannot_join_or_be_overwritten_by_old_probe() {
    for save_configuration in [false, true] {
        let fixture = Fixture::new();
        let old = fixture.program("old", "1.0.0", true);
        let new = fixture.program("new", "2.0.0", false);
        *fixture.paths.lock().unwrap() = vec![old.parent().unwrap().into()];
        let first = tokio::spawn(check(fixture.core.clone()));
        tokio::time::timeout(Duration::from_secs(10), async {
            while !fixture.root.join("probe-started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if save_configuration {
            fixture
                .save(
                    0,
                    &configuration(Some(&new), "new-saved", old.parent().unwrap()),
                )
                .await;
        } else {
            *fixture.paths.lock().unwrap() = vec![new.parent().unwrap().into()];
        }
        let second = tokio::spawn(check(fixture.core.clone()));
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.captures.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !first.is_finished(),
            "save/refresh must not stop the existing probe"
        );
        std::fs::write(fixture.root.join("probe-release"), b"release").unwrap();
        assert_eq!(first.await.unwrap(), RuntimeCheckOutcome::Superseded);
        assert_eq!(second.await.unwrap(), RuntimeCheckOutcome::StableFailure);
        assert_eq!(fixture.health().await["reportedVersion"], "codex-cli 2.0.0");
        assert_eq!(
            fixture.settings().await["revision"],
            u64::from(save_configuration)
        );
        fixture.close().await;
    }
}

// Owns the save/refresh lock interleaving and the absence of indirect operations.
// The existing probe race starts after environment capture and cannot cover this.
#[tokio::test]
async fn local_saves_finish_while_environment_capture_is_held_and_never_launch_work() {
    let (started, ready) = oneshot::channel();
    let started = StdMutex::new(Some(started));
    let (release, blocked) = std::sync::mpsc::channel();
    let blocked = StdMutex::new(blocked);
    let captures = Arc::new(AtomicUsize::new(0));
    let count = captures.clone();
    let fixture = Fixture::with_capture(Some(Arc::new(move |generation, _| {
        assert_eq!(
            count.fetch_add(1, Ordering::SeqCst),
            0,
            "save must not capture the environment"
        );
        started.lock().unwrap().take().unwrap().send(()).unwrap();
        blocked.lock().unwrap().recv().unwrap();
        RuntimeSearchEnvironment::for_test_paths(generation, Vec::new())
    })));
    let program = fixture.root.join(if cfg!(windows) {
        "offline.exe"
    } else {
        "offline"
    });
    std::fs::write(
        &program,
        b"synthetic native executable: must never be launched",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let refresh = {
        let core = fixture.core.clone();
        tokio::spawn(async move { core.refresh_runtime_check_environment(true).await })
    };
    ready.await.unwrap();
    let discovery = serde_json::to_value(&*fixture.core.runtime_discovery.read().await).unwrap();
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli] {
        let home = fixture.root.join(kind.as_str());
        std::fs::create_dir_all(&home).unwrap();
        let file = home.join(if kind == AdapterKind::CodexCli {
            "config.toml"
        } else {
            "settings.json"
        });
        let initial = if kind == AdapterKind::CodexCli {
            "model_provider='relay'\nmodel='first'\nkeep_unknown=42\n[model_providers.relay]\nbase_url='https://offline.invalid'\nexperimental_bearer_token='fixture-key'\n".to_string()
        } else {
            json!({"keep_unknown":42,"env":{"ANTHROPIC_BASE_URL":"https://offline.invalid","ANTHROPIC_AUTH_TOKEN":"fixture-key"}}).to_string()
        };
        std::fs::write(&file, initial).unwrap();
        let configuration = RuntimeStartupConfiguration {
            program_path: Some(program.to_string_lossy().into_owned()),
            environment: vec![RuntimeEnvironmentVariable {
                name: if kind == AdapterKind::CodexCli {
                    "CODEX_HOME"
                } else {
                    "CLAUDE_CONFIG_DIR"
                }
                .into(),
                value: home.to_string_lossy().into_owned(),
            }],
            ..Default::default()
        };
        rovai_core::runtime_startup::save(
            &mut *fixture.core.database.lock().await,
            kind,
            0,
            configuration,
            1,
        )
        .unwrap();
        let old = rovai_core::runtime_startup::load(&*fixture.core.database.lock().await, kind)
            .unwrap()
            .configuration
            .custom_api_snapshot
            .unwrap();
        for operation in ["url", "replace", "model", "environment", "clear", "program"] {
            let current = fixture
                .core
                .handle_runtime_startup("runtime.startup.get", json!({"runtimeKind":kind}))
                .await
                .unwrap();
            let config: RuntimeStartupConfiguration =
                serde_json::from_value(current["configuration"].clone()).unwrap();
            let editable = rovai_core::runtime_startup::editable(kind, &config);
            let (path, before, after, key) = match operation {
                "url" => (
                    json!(["baseUrl"]),
                    editable["baseUrl"].clone(),
                    json!("https://still-offline.invalid"),
                    json!({"action":"keep"}),
                ),
                "replace" | "clear" => (
                    json!(["credentialVersion"]),
                    current["credential"]["version"].clone(),
                    json!(operation),
                    if operation == "replace" {
                        json!({"action":"replace","value":"replacement-fixture-key"})
                    } else {
                        json!({"action":"clear"})
                    },
                ),
                "model" if kind == AdapterKind::ClaudeCodeCli => (
                    json!(["claudeModels", "model"]),
                    editable["claudeModels"]["model"].clone(),
                    json!("offline-model"),
                    json!({"action":"keep"}),
                ),
                "model" => {
                    let row = editable["codexModels"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .next()
                        .unwrap()
                        .clone();
                    (
                        json!(["codexModels", row, "id"]),
                        editable["codexModels"][&row]["id"].clone(),
                        json!("offline-model"),
                        json!({"action":"keep"}),
                    )
                }
                "environment" => (
                    json!(["environment", "VISIBLE_LOCAL"]),
                    Value::Null,
                    json!("updated"),
                    json!({"action":"keep"}),
                ),
                _ => (
                    json!(["programPath"]),
                    editable["programPath"].clone(),
                    Value::Null,
                    json!({"action":"keep"}),
                ),
            };
            let mut edits =
                vec![json!({"path":path,"before":before,"after":after,"label":"fixture"})];
            if !matches!(operation, "environment" | "program") {
                edits.push(json!({"path":["nativeRevision"],"before":current["nativeRevision"],"after":current["nativeRevision"],"label":"source"}));
            }
            let saved = tokio::time::timeout(
                Duration::from_secs(2),
                fixture.core.handle_runtime_startup(
                    "runtime.startup.save",
                    json!({"runtimeKind":kind,"edits":edits,"apiKey":key}),
                ),
            )
            .await
            .expect("a blocked reader must not delay local commit")
            .unwrap();
            assert!(
                saved.get("status").is_none(),
                "unexpected conflict: {operation}"
            );
            assert!(saved["credential"].get("value").is_none());
            assert!(!saved.to_string().contains("fixture-key"));
            assert_eq!(captures.load(Ordering::SeqCst), 1);
            assert_eq!(
                serde_json::to_value(&*fixture.core.runtime_discovery.read().await).unwrap(),
                discovery
            );
            assert!(
                !refresh.is_finished(),
                "save cannot release/restart the existing reader"
            );
        }
        assert!(
            old.assert_current().is_err(),
            "old connection snapshots cannot be reused by a later Run"
        );
        if kind == AdapterKind::CodexCli {
            // Restore-auto has no confirmed launcher in this isolated fixture.
            // A following API save must not guess the default file or discover
            // another executable, even though that local file is readable.
            let before = std::fs::read(&file).unwrap();
            let rejected = fixture
                .core
                .handle_runtime_startup(
                    "runtime.startup.save",
                    json!({"runtimeKind":kind,"edits":[{
                    "path":["baseUrl"],"before":"https://still-offline.invalid",
                    "after":"https://unconfirmed.invalid","label":"URL"
                }],"apiKey":{"action":"keep"}}),
                )
                .await
                .unwrap_err();
            assert!(rejected.to_string().contains("目标尚未确认"));
            assert_eq!(std::fs::read(&file).unwrap(), before);

            // An independent owner read can supply the known automatic
            // launcher. Later saves reuse it, until PATH selects another source.
            let configuration = fixture
                .core
                .runtime_search_environment
                .read()
                .await
                .startup_configuration(kind);
            rovai_core::runtime_custom_api::native::with_save_launcher(
                Some(program.to_string_lossy().into_owned()),
                &configuration,
                fixture
                    .core
                    .handle_runtime_startup("runtime.startup.get", json!({"runtimeKind":kind})),
            )
            .await
            .unwrap();
            let confirmed = fixture.core.handle_runtime_startup("runtime.startup.save", json!({
                "runtimeKind":kind,"edits":[{"path":["baseUrl"],"before":"https://still-offline.invalid",
                    "after":"https://confirmed.invalid","label":"URL"}],"apiKey":{"action":"keep"}
            })).await.unwrap();
            assert_eq!(
                confirmed["configuration"]["customApi"]["baseUrl"],
                "https://confirmed.invalid"
            );
            fixture.core.handle_runtime_startup("runtime.startup.save", json!({
                "runtimeKind":kind,"edits":[{"path":["environment","PATH"],"before":null,
                    "after":"/unconfirmed-runtime-path","label":"PATH"}],"apiKey":{"action":"keep"}
            })).await.unwrap();
            let rejected = fixture.core.handle_runtime_startup("runtime.startup.save", json!({
                "runtimeKind":kind,"edits":[{"path":["baseUrl"],"before":"https://confirmed.invalid",
                    "after":"https://wrong-source.invalid","label":"URL"}],"apiKey":{"action":"keep"}
            })).await.unwrap_err();
            assert!(rejected.to_string().contains("目标尚未确认"));
            assert_eq!(captures.load(Ordering::SeqCst), 1);
            assert!(!refresh.is_finished());
        }
        assert!(
            std::fs::read_to_string(file)
                .unwrap()
                .contains("keep_unknown")
        );
    }
    let generation = fixture
        .core
        .runtime_search_environment
        .read()
        .await
        .generation();
    release.send(()).unwrap();
    let published = refresh.await.unwrap().unwrap();
    assert!(published.generation() > generation);
    for kind in [AdapterKind::ClaudeCodeCli, AdapterKind::CodexCli] {
        assert!(
            published
                .startup_configuration(kind)
                .environment
                .iter()
                .any(|entry| entry.name == "VISIBLE_LOCAL" && entry.value == "updated"),
            "late refresh must merge the latest saved configuration"
        );
    }
    fixture.close().await;
}

#[tokio::test]
async fn failed_environment_reader_cannot_prevent_a_local_save() {
    let mut fixture = Fixture::new();
    fixture.shutdown.take().unwrap().send(()).unwrap();
    fixture.manager.take().unwrap().await.unwrap();
    let mut core = Arc::try_unwrap(fixture.core).ok().unwrap();
    core.runtime_search_capture = Some(Arc::new(|_, _| {
        panic!("synthetic environment reader failure")
    }));
    let before = core.runtime_search_environment.read().await.summary();
    assert!(core.refresh_runtime_check_environment(true).await.is_err());
    assert!(
        core.inspect_runtime_startup(KIND, RuntimeStartupConfiguration::default(), false)
            .await
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(core.runtime_search_environment.read().await.summary()).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    let saved = core
        .handle_runtime_startup(
            "runtime.startup.save",
            json!({
                "runtimeKind": KIND, "expectedRevision": 0,
                "configuration": RuntimeStartupConfiguration::default(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(saved["revision"], 1);
    assert_eq!(
        serde_json::to_value(core.runtime_search_environment.read().await.summary().shell).unwrap(),
        serde_json::to_value(before.shell).unwrap(),
        "save preserves the captured environment instead of retrying its failed reader"
    );
    drop(core);
    std::fs::remove_dir_all(fixture.root).unwrap();
}
