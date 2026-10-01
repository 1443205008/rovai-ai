//! Numeric Usage from verified, root-agent native journals. No journal content
//! is retained or forwarded; a prompt starts with a cursor and identity baseline.
use std::{
    collections::BTreeSet,
    fs::{self, File, Metadata, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    agent_profile::AdapterKind,
    monitoring::{
        ParsedRuntimeUsage, RuntimeInputSemantics, RuntimeUsageCounterMode, RuntimeUsageFields,
    },
};

const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINE_BYTES: u64 = 1024 * 1024;
const MAX_IDENTITIES: usize = 8192;

#[derive(Clone, Copy, Debug)]
enum Dialect {
    CodeBuddy,
    Kimi,
    Qoder,
}

pub(crate) struct NativeUsageObservation {
    pub source_identity: String,
    pub usage: ParsedRuntimeUsage,
}

pub(crate) enum NativeUsageReader {
    Jsonl(NativeJsonlUsageReader),
    OpenCode(OpenCodeUsageReader),
}

impl NativeUsageReader {
    pub(crate) fn for_prompt(
        kind: AdapterKind,
        version: Option<&str>,
        workspace: &Path,
        session_id: &str,
    ) -> Option<Self> {
        if kind == AdapterKind::OpencodeCli
            && crate::monitoring::reported_version_is(version, [1, 18, 30])
        {
            OpenCodeUsageReader::for_prompt(workspace, session_id).map(Self::OpenCode)
        } else {
            NativeJsonlUsageReader::for_prompt(kind, version, workspace, session_id)
                .map(Self::Jsonl)
        }
    }
    pub(crate) fn poll(&mut self) -> Vec<NativeUsageObservation> {
        match self {
            Self::Jsonl(reader) => reader.poll(),
            Self::OpenCode(reader) => reader.poll(),
        }
    }

    pub(crate) fn poll_prompt_end(&mut self) -> Vec<NativeUsageObservation> {
        let mut observed = self.poll();
        // Kimi's step.end journal append can follow the ACP response. Keep the
        // current Run's cursor until this bounded tail read; a successor prompt
        // may only establish its baseline after the terminal flush completes.
        std::thread::sleep(std::time::Duration::from_millis(400));
        observed.extend(self.poll());
        observed
    }
}

pub(crate) struct NativeJsonlUsageReader {
    dialect: Dialect,
    root: PathBuf,
    path: PathBuf,
    workspace: String,
    session_id: String,
    offset: u64,
    file_identity: Option<FileIdentity>,
    skip_partial: bool,
    seen: BTreeSet<String>,
    disabled: bool,
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                dev: metadata.dev(),
                ino: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                created: metadata.created().ok(),
            }
        }
    }
}

impl NativeJsonlUsageReader {
    pub(crate) fn for_prompt(
        kind: AdapterKind,
        version: Option<&str>,
        workspace: &Path,
        session_id: &str,
    ) -> Option<Self> {
        // These are installed-version witnesses, not a promise about future
        // journal formats. Other versions continue using their wire dialect.
        let (dialect, env_key, default_home) = match (
            kind,
            version.and_then(crate::monitoring::parse_reported_version),
        ) {
            (AdapterKind::CodebuddyCli, Some([2, 133, 1])) => {
                (Dialect::CodeBuddy, "CODEBUDDY_CONFIG_DIR", ".codebuddy")
            }
            (AdapterKind::KimiCodeCli, Some([2, 1, 1])) => {
                (Dialect::Kimi, "KIMI_CODE_HOME", ".kimi-code")
            }
            (AdapterKind::QoderCli, Some([1, 1, 64])) => {
                (Dialect::Qoder, "QODER_CONFIG_DIR", ".qoder")
            }
            _ => return None,
        };
        if !safe_identity(session_id) || session_id.starts_with("agent-") {
            return None;
        }
        let home = std::env::var_os(env_key)
            .filter(|s| !s.to_string_lossy().trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(default_home)))?;
        let home = if home.is_absolute() {
            home
        } else {
            workspace.join(home)
        };
        let root = fs::canonicalize(home).ok()?;
        let workspace = workspace.to_str()?.to_string();
        let path = match dialect {
            Dialect::CodeBuddy => root
                .join("projects")
                .join(codebuddy_workspace_key(&workspace))
                .join(format!("{session_id}.jsonl")),
            Dialect::Kimi => root
                .join("sessions")
                .join(kimi_workspace_key(&workspace))
                .join(session_id)
                .join("agents/main/wire.jsonl"),
            Dialect::Qoder => root
                .join("projects")
                .join(qoder_workspace_key(&workspace))
                .join(format!("{session_id}.jsonl")),
        };
        Self::baseline(dialect, root, path, workspace, session_id.to_string()).ok()
    }

    fn baseline(
        dialect: Dialect,
        root: PathBuf,
        path: PathBuf,
        workspace: String,
        session_id: String,
    ) -> Result<Self> {
        let mut reader = Self {
            dialect,
            root,
            path,
            workspace,
            session_id,
            offset: 0,
            file_identity: None,
            skip_partial: false,
            seen: BTreeSet::new(),
            disabled: false,
        };
        if let Some(mut file) = reader.open()? {
            let metadata = file.metadata()?;
            if metadata.len() > MAX_JOURNAL_BYTES {
                bail!("native Usage baseline exceeds bound");
            }
            reader.file_identity = Some(FileIdentity::from_metadata(&metadata));
            let end = metadata.len();
            reader.read_complete_lines(&mut file, end, true)?;
            if reader.disabled {
                bail!("native Usage baseline is incomplete");
            }
            // An unfinished historical line is never completed into new Usage.
            reader.skip_partial = reader.offset < end;
            reader.offset = end;
        }
        Ok(reader)
    }

    fn open(&self) -> Result<Option<File>> {
        let relative = self.path.strip_prefix(&self.root)?;
        let mut path = self.root.clone();
        for component in relative.components() {
            if !matches!(component, Component::Normal(_)) {
                bail!("native Usage path is invalid");
            }
            path.push(component);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => bail!("native Usage path is a link"),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => bail!("native Usage journal is unavailable"),
            }
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options
            .open(&self.path)
            .map_err(|_| anyhow::anyhow!("native Usage journal is unavailable"))?;
        if !file.metadata()?.is_file() {
            bail!("native Usage journal is not a regular file");
        }
        Ok(Some(file))
    }

    pub(crate) fn poll(&mut self) -> Vec<NativeUsageObservation> {
        if self.disabled {
            return Vec::new();
        }
        match self.poll_checked() {
            Ok(observations) => observations,
            Err(_) => {
                // A reset, replacement or gap cannot restart from byte zero and
                // silently claim historical consumption for this prompt.
                self.disabled = true;
                Vec::new()
            }
        }
    }

    fn poll_checked(&mut self) -> Result<Vec<NativeUsageObservation>> {
        let Some(mut file) = self.open()? else {
            if self.file_identity.is_some() {
                bail!("native Usage journal disappeared");
            }
            return Ok(Vec::new());
        };
        let metadata = file.metadata()?;
        let identity = FileIdentity::from_metadata(&metadata);
        if metadata.len() < self.offset
            || metadata.len() > MAX_JOURNAL_BYTES
            || self
                .file_identity
                .as_ref()
                .is_some_and(|old| old != &identity)
        {
            bail!("native Usage journal continuity lost");
        }
        self.file_identity = Some(identity);
        file.seek(SeekFrom::Start(self.offset))?;
        self.read_complete_lines(&mut file, metadata.len(), false)
    }

    fn read_complete_lines(
        &mut self,
        file: &mut File,
        end: u64,
        baseline: bool,
    ) -> Result<Vec<NativeUsageObservation>> {
        let mut input = BufReader::new(file.take(end - self.offset));
        let mut observations = Vec::new();
        loop {
            let mut line = Vec::new();
            let count = match input
                .by_ref()
                .take(MAX_LINE_BYTES + 1)
                .read_until(b'\n', &mut line)
            {
                Ok(count) => count,
                Err(_) => {
                    self.disabled = true;
                    break;
                }
            };
            if count == 0 {
                break;
            }
            if count as u64 > MAX_LINE_BYTES {
                self.disabled = true;
                break;
            }
            if line.last() != Some(&b'\n') {
                break;
            }
            self.offset += count as u64;
            if self.skip_partial {
                self.skip_partial = false;
                continue;
            }
            let incoming = match self.dialect {
                Dialect::CodeBuddy => parse_codebuddy(&line, &self.session_id, &self.workspace)
                    .into_iter()
                    .collect(),
                Dialect::Kimi => parse_kimi(&line, &self.session_id).into_iter().collect(),
                Dialect::Qoder => {
                    let Some(record) = qoder_record(&line, &self.session_id, &self.workspace)
                    else {
                        continue;
                    };
                    // Qoder appends incomplete assistant snapshots before their
                    // Usage. An old pending message must also belong to the
                    // baseline, even if its final Usage arrives after this prompt.
                    let baseline_id =
                        format!("qoder-baseline:{}:{}", self.session_id, record.message.id);
                    if baseline {
                        if self.seen.len() >= MAX_IDENTITIES {
                            self.disabled = true;
                            break;
                        }
                        self.seen.insert(baseline_id);
                        continue;
                    }
                    if self.seen.contains(&baseline_id) {
                        continue;
                    }
                    parse_qoder(record, &self.session_id)
                }
            };
            for observation in incoming {
                if self.seen.contains(&observation.source_identity) {
                    continue;
                }
                if self.seen.len() >= MAX_IDENTITIES {
                    self.disabled = true;
                    break;
                }
                self.seen.insert(observation.source_identity.clone());
                if !baseline {
                    observations.push(observation);
                }
            }
            if self.disabled {
                break;
            }
        }
        Ok(observations)
    }
}

pub(crate) struct OpenCodeUsageReader {
    path: PathBuf,
    workspace: String,
    session_id: String,
    file_identity: FileIdentity,
    seen: BTreeSet<String>,
    disabled: bool,
}

impl OpenCodeUsageReader {
    fn for_prompt(workspace: &Path, session_id: &str) -> Option<Self> {
        if !safe_identity(session_id) {
            return None;
        }
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")))?;
        if !data.is_absolute() {
            return None;
        }
        let root = fs::canonicalize(data.join("opencode")).ok()?;
        let path = root.join("opencode.db");
        Self::baseline(
            path,
            workspace.to_str()?.to_string(),
            session_id.to_string(),
        )
        .ok()
    }

    fn baseline(path: PathBuf, workspace: String, session_id: String) -> Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("native Usage database is not a regular file");
        }
        let mut reader = Self {
            path,
            workspace,
            session_id,
            file_identity: FileIdentity::from_metadata(&metadata),
            seen: BTreeSet::new(),
            disabled: false,
        };
        let database = reader.open()?;
        reader.validate_session(&database)?;
        let mut query = database.prepare("SELECT id FROM message WHERE session_id=?1 LIMIT ?2")?;
        let ids = query.query_map(
            rusqlite::params![reader.session_id, (MAX_IDENTITIES + 1) as i64],
            |row| row.get::<_, String>(0),
        )?;
        for id in ids {
            reader.seen.insert(id?);
        }
        if reader.seen.len() > MAX_IDENTITIES {
            bail!("native Usage baseline exceeds identity bound");
        }
        Ok(reader)
    }

    fn open(&self) -> Result<rusqlite::Connection> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || FileIdentity::from_metadata(&metadata) != self.file_identity
        {
            bail!("native Usage database continuity lost");
        }
        let database = rusqlite::Connection::open_with_flags(
            &self.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        database.busy_timeout(std::time::Duration::from_millis(250))?;
        Ok(database)
    }

    fn validate_session(&self, database: &rusqlite::Connection) -> Result<()> {
        let (directory, parent) = database.query_row(
            "SELECT directory,parent_id FROM session WHERE id=?1",
            [&self.session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        if directory != self.workspace || parent.is_some() {
            bail!("native Usage root Session does not match");
        }
        Ok(())
    }

    fn poll(&mut self) -> Vec<NativeUsageObservation> {
        if self.disabled {
            return Vec::new();
        }
        match self.poll_checked() {
            Ok(value) => value,
            Err(_) => {
                self.disabled = true;
                Vec::new()
            }
        }
    }

    fn poll_checked(&mut self) -> Result<Vec<NativeUsageObservation>> {
        let database = self.open()?;
        self.validate_session(&database)?;
        // Only metadata scalars leave SQLite. Native parts and message bodies
        // are never selected into Core memory or its public Evidence channel.
        let mut query = database.prepare(
            "SELECT id,
            json_extract(data,'$.tokens.input'), json_extract(data,'$.tokens.output'),
            json_extract(data,'$.tokens.reasoning'), json_extract(data,'$.tokens.cache.read'),
            json_extract(data,'$.tokens.cache.write'), json_extract(data,'$.time.completed')
            FROM message WHERE session_id=?1 AND json_valid(data)
                AND json_extract(data,'$.role')='assistant'
                AND json_type(data,'$.time.completed')='integer'
            ORDER BY time_created,id LIMIT ?2",
        )?;
        let rows = query.query_map(
            rusqlite::params![self.session_id, (MAX_IDENTITIES + 1) as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    RuntimeUsageFields {
                        input_tokens: row.get(1)?,
                        output_tokens: row.get(2)?,
                        reasoning_output_tokens: row.get(3)?,
                        cache_read_input_tokens: row.get(4)?,
                        cache_write_input_tokens: row.get(5)?,
                        ..Default::default()
                    },
                    row.get::<_, Option<i64>>(6)?,
                ))
            },
        )?;
        let mut result = Vec::new();
        for row in rows {
            let (id, mut fields, time) = row?;
            if self.seen.contains(&id) {
                continue;
            }
            if self.seen.len() >= MAX_IDENTITIES || !safe_identity(&id) {
                self.disabled = true;
                break;
            }
            self.seen.insert(id.clone());
            fields.output_tokens = match (fields.output_tokens, fields.reasoning_output_tokens) {
                (Some(output), Some(reasoning)) if output >= 0 && reasoning >= 0 => {
                    output.checked_add(reasoning)
                }
                _ => None,
            };
            result.push(observation(
                &self.session_id,
                format!("opencode-native:{}:{id}", self.session_id),
                "opencode-native-message-usage-1.18.30",
                None,
                RuntimeInputSemantics::ExclusiveBuckets,
                fields.clone(),
                time,
            ));
            // The installed ACPUsage.contextTokens() defines occupancy as the
            // latest call's input + both cache buckets, excluding output. ACP
            // supplies size from its effective Provider/Model catalog when
            // known. Preserve used alone when the native catalog has no limit.
            let used = fields
                .input_tokens
                .zip(fields.cache_read_input_tokens)
                .zip(fields.cache_write_input_tokens)
                .filter(|((input, read), write)| *input >= 0 && *read >= 0 && *write >= 0)
                .and_then(|((input, read), write)| input.checked_add(read)?.checked_add(write));
            if let Some(used) = used {
                let mut gauge = observation(
                    &self.session_id,
                    format!("opencode-context:{}:{id}", self.session_id),
                    "opencode-native-call-context-1.18.30",
                    None,
                    RuntimeInputSemantics::Unknown,
                    RuntimeUsageFields {
                        context_used_tokens: Some(used),
                        ..Default::default()
                    },
                    time,
                );
                gauge.usage.scope = "session".into();
                gauge.usage.counter_mode = RuntimeUsageCounterMode::Gauge;
                gauge.usage.identity_suffix = "native_context".into();
                result.push(gauge);
            }
        }
        Ok(result)
    }
}

fn safe_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

fn codebuddy_workspace_key(workspace: &str) -> String {
    workspace
        .split(['/', '\\', ':', '-'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn qoder_workspace_key(workspace: &str) -> String {
    workspace
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn kimi_workspace_key(workspace: &str) -> String {
    let normalized = workspace.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let mut slug = String::new();
    for c in normalized
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_lowercase()
        .chars()
    {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').chars().take(40).collect::<String>();
    let slug = slug.trim_matches('-');
    let slug = if matches!(slug, "" | "." | "..") {
        "workspace"
    } else {
        slug
    };
    let digest = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    format!("wd_{slug}_{}", &digest[..12])
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeBuddyRecord {
    #[serde(rename = "type")]
    kind: String,
    role: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    timestamp: Option<i64>,
    provider_data: Option<CodeBuddyProvider>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeBuddyProvider {
    message_id: Option<String>,
    agent: Option<String>,
    raw_usage: Option<OpenAiUsage>,
}
#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    prompt_tokens_details: Option<PromptDetails>,
    completion_tokens_details: Option<CompletionDetails>,
}
#[derive(Deserialize)]
struct PromptDetails {
    cached_tokens: Option<i64>,
}
#[derive(Deserialize)]
struct CompletionDetails {
    reasoning_tokens: Option<i64>,
}

fn nonnegative(value: Option<i64>) -> Option<i64> {
    value.filter(|n| *n >= 0)
}

fn observation(
    session_id: &str,
    identity: String,
    dialect: &str,
    turn: Option<String>,
    semantics: RuntimeInputSemantics,
    fields: RuntimeUsageFields,
    time: Option<i64>,
) -> NativeUsageObservation {
    NativeUsageObservation {
        source_identity: identity,
        usage: ParsedRuntimeUsage {
            identity_suffix: "native_model_call".to_string(),
            dialect_id: dialect.to_string(),
            source: "runtime_private_extension".to_string(),
            scope: "model_call".to_string(),
            counter_mode: RuntimeUsageCounterMode::Delta,
            input_semantics: semantics,
            native_session_id: Some(session_id.to_string()),
            native_turn_id: turn,
            fields,
            cost: None,
            occurred_at: time
                .and_then(DateTime::<Utc>::from_timestamp_millis)
                .map(|t| t.to_rfc3339()),
        },
    }
}

fn parse_codebuddy(
    line: &[u8],
    session_id: &str,
    workspace: &str,
) -> Option<NativeUsageObservation> {
    let record: CodeBuddyRecord = serde_json::from_slice(line).ok()?;
    if record.session_id.as_deref() != Some(session_id)
        || record.cwd.as_deref() != Some(workspace)
        || !(record.kind == "function_call"
            || (record.kind == "message" && record.role.as_deref() == Some("assistant")))
    {
        return None;
    }
    let provider = record.provider_data?;
    if provider.agent.as_deref() != Some("cli") {
        return None;
    }
    let id = provider.message_id.filter(|id| safe_identity(id))?;
    let raw = provider.raw_usage?;
    let output = nonnegative(raw.completion_tokens);
    Some(observation(
        session_id,
        format!("codebuddy-native:{session_id}:{id}"),
        "codebuddy-native-raw-usage-2.133.1",
        None,
        RuntimeInputSemantics::CacheInclusiveTotal,
        RuntimeUsageFields {
            input_tokens: nonnegative(raw.prompt_tokens),
            output_tokens: output,
            cache_read_input_tokens: raw
                .prompt_tokens_details
                .and_then(|d| nonnegative(d.cached_tokens)),
            reasoning_output_tokens: raw
                .completion_tokens_details
                .and_then(|d| nonnegative(d.reasoning_tokens))
                .filter(|n| output.is_some_and(|out| *n <= out)),
            ..RuntimeUsageFields::default()
        },
        record.timestamp,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiRecord {
    #[serde(rename = "type")]
    kind: String,
    agent_id: Option<String>,
    event: Option<KimiEvent>,
    time: Option<i64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiEvent {
    #[serde(rename = "type")]
    kind: String,
    uuid: Option<String>,
    turn_id: Option<String>,
    usage: Option<KimiUsage>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiUsage {
    input_other: Option<i64>,
    output: Option<i64>,
    input_cache_read: Option<i64>,
    input_cache_creation: Option<i64>,
}

fn parse_kimi(line: &[u8], session_id: &str) -> Option<NativeUsageObservation> {
    let record: KimiRecord = serde_json::from_slice(line).ok()?;
    if record.kind != "context.append_loop_event" || record.agent_id.as_deref() != Some("main") {
        return None;
    }
    let event = record.event?;
    if event.kind != "step.end" {
        return None;
    }
    let id = event.uuid.filter(|id| safe_identity(id))?;
    let raw = event.usage?;
    Some(observation(
        session_id,
        format!("kimi-native:{session_id}:{id}"),
        "kimi-native-step-usage-2.1.1",
        event.turn_id,
        RuntimeInputSemantics::ExclusiveBuckets,
        RuntimeUsageFields {
            input_tokens: nonnegative(raw.input_other),
            output_tokens: nonnegative(raw.output),
            cache_read_input_tokens: nonnegative(raw.input_cache_read),
            cache_write_input_tokens: nonnegative(raw.input_cache_creation),
            ..RuntimeUsageFields::default()
        },
        record.time,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QoderRecord {
    #[serde(rename = "type")]
    kind: String,
    session_id: String,
    cwd: String,
    is_sidechain: bool,
    entrypoint: String,
    model_source: Option<String>,
    timestamp: String,
    message: QoderMessage,
}
#[derive(Deserialize)]
struct QoderMessage {
    id: String,
    role: String,
    usage: Option<QoderUsage>,
}
#[derive(Deserialize)]
struct QoderUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    context_usage_ratio: Option<f64>,
}

fn qoder_record(line: &[u8], session_id: &str, workspace: &str) -> Option<QoderRecord> {
    let record: QoderRecord = serde_json::from_slice(line).ok()?;
    (record.kind == "assistant"
        && record.message.role == "assistant"
        && record.entrypoint == "acp"
        && !record.is_sidechain
        && record.session_id == session_id
        && record.cwd == workspace
        && safe_identity(&record.message.id)
        && DateTime::parse_from_rfc3339(&record.timestamp).is_ok())
    .then_some(record)
}

fn parse_qoder(record: QoderRecord, session_id: &str) -> Vec<NativeUsageObservation> {
    let Some(raw) = record.message.usage else {
        return Vec::new();
    };
    let mut result = Vec::new();
    if record.model_source.as_deref() == Some("custom") {
        // Installed custom-provider Ag() copies OpenAI prompt_tokens into
        // input_tokens, including cache. The native redaction path for other
        // model sources replaces counts with zeros; it cannot establish Usage.
        let fields = RuntimeUsageFields {
            input_tokens: nonnegative(raw.input_tokens),
            output_tokens: nonnegative(raw.output_tokens),
            // Native normalization can synthesize zero for missing buckets.
            // Without original presence flags only positive caches are proven.
            cache_read_input_tokens: nonnegative(raw.cache_read_input_tokens).filter(|n| *n > 0),
            cache_write_input_tokens: nonnegative(raw.cache_creation_input_tokens)
                .filter(|n| *n > 0),
            ..Default::default()
        };
        if fields.input_tokens.is_some()
            || fields.output_tokens.is_some()
            || fields.cache_read_input_tokens.is_some()
            || fields.cache_write_input_tokens.is_some()
        {
            let mut item = observation(
                session_id,
                format!("qoder-native:{session_id}:{}", record.message.id),
                "qoder-custom-message-usage-1.1.64",
                None,
                RuntimeInputSemantics::CacheInclusiveTotal,
                fields,
                None,
            );
            item.usage.occurred_at = Some(record.timestamp.clone());
            result.push(item);
        }
    }
    if let Some(ratio) = raw
        .context_usage_ratio
        .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
    {
        let mut item = observation(
            session_id,
            format!(
                "qoder-context:{session_id}:{}:{}",
                record.message.id, record.timestamp
            ),
            "qoder-native-context-ratio-1.1.64",
            None,
            RuntimeInputSemantics::Unknown,
            RuntimeUsageFields {
                native_context_ratio: Some(ratio),
                ..Default::default()
            },
            None,
        );
        item.usage.scope = "session".into();
        item.usage.counter_mode = RuntimeUsageCounterMode::Gauge;
        item.usage.identity_suffix = "native_context".into();
        item.usage.occurred_at = Some(record.timestamp);
        result.push(item);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codebuddy(id: &str) -> serde_json::Value {
        json!({"type":"message","role":"assistant","sessionId":"session-1","cwd":"/workspace",
            "providerData":{"agent":"cli","messageId":id,"rawUsage":{"prompt_tokens":123,"completion_tokens":9},
            "usage":{"input_tokens":9999}},"content":"NATIVE_PRIVATE_CANARY"})
    }
    fn kimi(id: &str) -> serde_json::Value {
        json!({"type":"context.append_loop_event","agentId":"main","event":{"type":"step.end","uuid":id,
            "turnId":"0","usage":{"inputOther":12,"output":3,"inputCacheRead":20,"inputCacheCreation":0}},
            "content":"NATIVE_PRIVATE_CANARY"})
    }

    fn qoder(id: &str) -> serde_json::Value {
        json!({"type":"assistant","sessionId":"session-1","cwd":"/workspace",
            "entrypoint":"acp","isSidechain":false,"modelSource":"custom",
            "timestamp":"2026-10-01T00:00:00Z","message":{"id":id,"role":"assistant",
            "content":"NATIVE_PRIVATE_CANARY","usage":{"input_tokens":123,"output_tokens":9,
            "cache_read_input_tokens":20,"cache_creation_input_tokens":0}}})
    }

    // New owner: the private journal's exact source, root identity and sparse
    // fields. ACP parser fixtures cannot exercise these persisted envelopes.
    #[test]
    fn native_dialects_select_root_calls_preserve_missing_and_ignore_restated_content() {
        let value = codebuddy("call-1");
        let parsed = parse_codebuddy(
            &serde_json::to_vec(&value).unwrap(),
            "session-1",
            "/workspace",
        )
        .unwrap();
        assert_eq!(parsed.usage.fields.input_tokens, Some(123));
        assert_eq!(parsed.usage.fields.output_tokens, Some(9));
        assert_eq!(parsed.usage.fields.cache_read_input_tokens, None);
        assert_eq!(parsed.usage.fields.cache_write_input_tokens, None);
        assert!(
            !serde_json::to_string(&parsed.usage)
                .unwrap()
                .contains("NATIVE_PRIVATE_CANARY")
        );
        for (key, replacement) in [
            ("agent", json!("subagent")),
            ("messageId", json!(null)),
            ("rawUsage", json!(null)),
        ] {
            let mut value = codebuddy("call-1");
            value["providerData"][key] = replacement;
            assert!(
                parse_codebuddy(
                    &serde_json::to_vec(&value).unwrap(),
                    "session-1",
                    "/workspace"
                )
                .is_none()
            );
        }
        assert!(
            parse_codebuddy(
                &serde_json::to_vec(&value).unwrap(),
                "other-session",
                "/workspace"
            )
            .is_none()
        );
        assert!(
            parse_codebuddy(
                &serde_json::to_vec(&value).unwrap(),
                "session-1",
                "/other-workspace"
            )
            .is_none()
        );
        let mut value = codebuddy("call-1");
        value["providerData"]["rawUsage"]["prompt_tokens_details"] = json!({"cached_tokens":0});
        value["providerData"]["rawUsage"]["completion_tokens_details"] =
            json!({"reasoning_tokens":7});
        let parsed = parse_codebuddy(
            &serde_json::to_vec(&value).unwrap(),
            "session-1",
            "/workspace",
        )
        .unwrap();
        assert_eq!(parsed.usage.fields.cache_read_input_tokens, Some(0));
        assert_eq!(parsed.usage.fields.output_tokens, Some(9));
        assert_eq!(parsed.usage.fields.reasoning_output_tokens, Some(7));
        let value = kimi("step-1");
        let parsed = parse_kimi(&serde_json::to_vec(&value).unwrap(), "session-1").unwrap();
        assert_eq!(
            parsed.usage.input_semantics,
            RuntimeInputSemantics::ExclusiveBuckets
        );
        assert_eq!(parsed.usage.fields.input_tokens, Some(12));
        assert_eq!(parsed.usage.fields.cache_write_input_tokens, Some(0));
        assert!(
            !serde_json::to_string(&parsed.usage)
                .unwrap()
                .contains("NATIVE_PRIVATE_CANARY")
        );
        for (key, value) in [("type", json!("usage.record")), ("agentId", json!("child"))] {
            let mut record = kimi("step-1");
            record[key] = value;
            assert!(parse_kimi(&serde_json::to_vec(&record).unwrap(), "session-1").is_none());
        }
        let mut value = kimi("step-1");
        value["event"]["usage"]
            .as_object_mut()
            .unwrap()
            .remove("inputCacheCreation");
        assert_eq!(
            parse_kimi(&serde_json::to_vec(&value).unwrap(), "session-1")
                .unwrap()
                .usage
                .fields
                .cache_write_input_tokens,
            None
        );
        assert_eq!(
            kimi_workspace_key(
                "/private/var/folders/pm/zmpfxggd0glcm8vx3p3y3mmr0000gq/T/rovai-observable-kimi-code-cli-QmbRQg/workspace"
            ),
            "wd_workspace_cbe2b4c8286f"
        );
        assert_eq!(
            codebuddy_workspace_key("/private/a---b/workspace/"),
            "private-a-b-workspace"
        );
        assert_eq!(
            qoder_workspace_key("/private/a_b/workspace"),
            "-private-a-b-workspace"
        );
        let mut frame = qoder("call-1");
        frame["message"]["usage"]["context_usage_ratio"] = json!(0.125);
        let parse = |v: &serde_json::Value| {
            qoder_record(&serde_json::to_vec(v).unwrap(), "session-1", "/workspace")
                .map(|r| parse_qoder(r, "session-1"))
                .unwrap_or_default()
        };
        let result = parse(&frame);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].usage.fields.input_tokens, Some(123));
        assert_eq!(
            result[0].usage.input_semantics,
            RuntimeInputSemantics::CacheInclusiveTotal
        );
        assert_eq!(
            result[0].usage.fields.cache_write_input_tokens, None,
            "native default zero has no provider presence evidence"
        );
        assert_eq!(result[1].usage.fields.native_context_ratio, Some(0.125));
        assert_eq!(result[1].usage.fields.context_used_tokens, None);
        assert_eq!(result[1].usage.fields.context_size_tokens, None);
        assert!(
            !serde_json::to_string(&result[0].usage)
                .unwrap()
                .contains("NATIVE_PRIVATE_CANARY")
        );
        frame["modelSource"] = json!("native");
        assert_eq!(
            parse(&frame).len(),
            1,
            "redacted counts cannot establish zero Usage; ratio is independent"
        );
        for ratio in [json!(-0.01), json!(1.01), json!(null)] {
            frame["message"]["usage"]["context_usage_ratio"] = ratio;
            assert!(parse(&frame).is_empty());
        }
        frame["message"]["usage"]["context_usage_ratio"] = json!(0);
        assert_eq!(
            parse(&frame)[0].usage.fields.native_context_ratio,
            Some(0.0)
        );
        for (key, value) in [
            ("isSidechain", json!(true)),
            ("sessionId", json!("other")),
            ("cwd", json!("/other")),
            ("entrypoint", json!("cli")),
        ] {
            let mut invalid = frame.clone();
            invalid[key] = value;
            assert!(parse(&invalid).is_empty());
        }
        let witness: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/research/runtime-monitoring/fixtures/round5-native-usage-context.json"
        ))
        .unwrap();
        // Replay sanitized installed-version fields, independently captured
        // before parsing, against their recorded per-call normalization.
        for entry in witness["entries"].as_array().unwrap() {
            let kind = entry["runtime"].as_str().unwrap();
            if !matches!(kind, "codebuddy-cli" | "kimi-code-cli") {
                continue;
            }
            for run in entry["runs"].as_array().unwrap() {
                for record in run["sourceRecords"].as_array().unwrap() {
                    let frame = serde_json::to_vec(&record["raw"]).unwrap();
                    let observed = if kind == "codebuddy-cli" {
                        parse_codebuddy(&frame, "session-1", "/workspace")
                    } else {
                        parse_kimi(&frame, "session-1")
                    }
                    .unwrap();
                    let fields = observed.usage.fields;
                    let total = if kind == "kimi-code-cli" {
                        fields.input_tokens.and_then(|input| {
                            Some(
                                input
                                    + fields.cache_read_input_tokens?
                                    + fields.cache_write_input_tokens?,
                            )
                        })
                    } else {
                        fields.input_tokens
                    };
                    assert_eq!(
                        json!({
                            "promptInputTotalTokens":total,"outputTokens":fields.output_tokens,
                            "cacheReadTokens":fields.cache_read_input_tokens,"cacheWriteTokens":fields.cache_write_input_tokens
                        }),
                        record["expectedParsed"],
                        "{kind} raw fixture must retain its independently recorded semantics"
                    );
                }
            }
        }
        for source in [
            include_str!(
                "../../../docs/research/runtime-monitoring/fixtures/round6-native-context-ratio.json"
            ),
            include_str!(
                "../../../docs/research/runtime-monitoring/fixtures/round7-native-boundaries.json"
            ),
        ] {
            let witness: serde_json::Value = serde_json::from_str(source).unwrap();
            let entry = witness["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["runtime"] == "qoder-cli")
                .unwrap();
            for run in entry["runs"].as_array().unwrap() {
                for record in run["sourceRecords"].as_array().unwrap() {
                    let frame = serde_json::to_vec(&record["raw"]).unwrap();
                    let observations = parse_qoder(
                        qoder_record(&frame, "session-1", "/workspace").unwrap(),
                        "session-1",
                    );
                    let fields = &observations[0].usage.fields;
                    assert_eq!(
                        json!({"promptInputTotalTokens":fields.input_tokens,
                        "outputTokens":fields.output_tokens,
                        "cacheReadTokens":fields.cache_read_input_tokens,
                        "cacheWriteTokens":fields.cache_write_input_tokens}),
                        record["expectedParsed"]
                    );
                    let context = &observations[1].usage.fields;
                    assert_eq!(
                        json!({"usedTokens":context.context_used_tokens,
                        "windowTokens":context.context_size_tokens,
                        "nativeRatio":context.native_context_ratio}),
                        record["expectedContext"]
                    );
                }
            }
        }
    }

    // New filesystem owner: byte/identity baseline, torn lines and continuity.
    // A parser alone cannot prove which bytes were already present at dispatch.
    #[cfg(feature = "extended-tests")]
    #[test]
    fn native_cursor_excludes_history_replays_partial_lines_and_file_resets() {
        use std::io::Write;
        for dialect in [Dialect::CodeBuddy, Dialect::Kimi, Dialect::Qoder] {
            let root =
                std::env::temp_dir().join(format!("rovai-native-usage-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let path = root.join("wire.jsonl");
            let make = |id| match dialect {
                Dialect::CodeBuddy => codebuddy(id),
                Dialect::Kimi => kimi(id),
                Dialect::Qoder => qoder(id),
            };
            let line = |id| format!("{}\n", make(id));
            let mut historical = make("old");
            if matches!(dialect, Dialect::Qoder) {
                historical["message"]
                    .as_object_mut()
                    .unwrap()
                    .remove("usage");
            }
            fs::write(&path, format!("{historical}\n")).unwrap();
            let mut reader = NativeJsonlUsageReader::baseline(
                dialect,
                root.clone(),
                path.clone(),
                "/workspace".to_string(),
                "session-1".to_string(),
            )
            .unwrap();
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            write!(file, "{}{}", line("old"), line("new").trim_end()).unwrap();
            file.flush().unwrap();
            assert!(
                reader.poll().is_empty(),
                "partial new frame waits for newline; history is excluded"
            );
            writeln!(file).unwrap();
            file.flush().unwrap();
            let first = reader.poll();
            assert_eq!(first.len(), 1);
            write!(file, "{}", line("new")).unwrap();
            file.flush().unwrap();
            assert!(
                reader.poll().is_empty(),
                "native repeat cannot claim another model call"
            );
            let mut next_run = NativeJsonlUsageReader::baseline(
                dialect,
                root.clone(),
                path.clone(),
                "/workspace".to_string(),
                "session-1".to_string(),
            )
            .unwrap();
            write!(file, "{}{}", line("new"), line("next-run")).unwrap();
            file.flush().unwrap();
            assert_eq!(
                next_run.poll().len(),
                1,
                "resume baseline claims only successor consumption"
            );
            drop(file);
            fs::write(&path, line("reset")).unwrap();
            assert!(next_run.poll().is_empty());
            assert!(next_run.disabled);
            fs::write(&path, line("old").trim_end()).unwrap();
            let mut partial = NativeJsonlUsageReader::baseline(
                dialect,
                root.clone(),
                path.clone(),
                "/workspace".to_string(),
                "session-1".to_string(),
            )
            .unwrap();
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            write!(file, "\n{}", line("fresh")).unwrap();
            file.flush().unwrap();
            assert_eq!(
                partial.poll().len(),
                1,
                "completing historical partial bytes is not new Usage"
            );
            drop(file);
            let mut terminal = NativeUsageReader::Jsonl(
                NativeJsonlUsageReader::baseline(
                    dialect,
                    root.clone(),
                    path.clone(),
                    "/workspace".into(),
                    "session-1".into(),
                )
                .unwrap(),
            );
            let late_path = path.clone();
            let late = line("late-terminal");
            let writer = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(75));
                let mut file = OpenOptions::new().append(true).open(late_path).unwrap();
                write!(file, "{late}").unwrap();
                file.flush().unwrap();
            });
            assert!(terminal.poll().is_empty());
            assert_eq!(
                terminal.poll_prompt_end().len(),
                1,
                "ACP completion may precede the native journal tail"
            );
            writer.join().unwrap();
            assert!(terminal.poll().is_empty());
            fs::remove_dir_all(root).unwrap();
        }
    }

    // SQLite metadata has a different continuity boundary from the append-only
    // journals: existing incomplete messages must remain history on resume.
    #[cfg(feature = "extended-tests")]
    #[test]
    fn opencode_metadata_excludes_old_pending_child_and_repeated_calls() {
        let root =
            std::env::temp_dir().join(format!("rovai-native-opencode-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let path = root.join("opencode.db");
        let database = rusqlite::Connection::open(&path).unwrap();
        database.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY,directory TEXT,parent_id TEXT);
            CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);
            INSERT INTO session VALUES('session-1','/workspace',NULL),('child','/workspace','session-1');").unwrap();
        let insert = |id: &str, session: &str, completed: Option<i64>| {
            database
                .execute(
                    "INSERT INTO message VALUES(?1,?2,1,?3)",
                    rusqlite::params![id, session, json!({
                "role":"assistant", "time":{"completed":completed},
                "tokens":{"input":12,"output":9,"reasoning":2,"cache":{"read":20,"write":0}},
                "content":"NATIVE_PRIVATE_CANARY"
            }).to_string()],
                )
                .unwrap();
        };
        insert("old-pending", "session-1", None);
        insert("old-completed", "session-1", Some(1));
        let mut reader =
            OpenCodeUsageReader::baseline(path.clone(), "/workspace".into(), "session-1".into())
                .unwrap();
        assert!(
            OpenCodeUsageReader::baseline(path.clone(), "/workspace".into(), "child".into())
                .is_err()
        );
        assert!(
            OpenCodeUsageReader::baseline(path.clone(), "/other".into(), "session-1".into())
                .is_err()
        );
        database.execute("UPDATE message SET data=json_set(data,'$.time.completed',2) WHERE id='old-pending'", []).unwrap();
        insert("child-call", "child", Some(2));
        insert("new-pending", "session-1", None);
        insert("new-call", "session-1", Some(3));
        let observations = reader.poll();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[1].usage.fields.context_used_tokens, Some(32));
        assert_eq!(observations[1].usage.fields.context_size_tokens, None);
        assert_eq!(
            observations[1].usage.counter_mode,
            RuntimeUsageCounterMode::Gauge
        );
        let fields = &observations[0].usage.fields;
        assert_eq!(fields.input_tokens, Some(12));
        assert_eq!(
            fields.output_tokens,
            Some(11),
            "native output excludes reasoning, add it once"
        );
        assert_eq!(fields.cache_write_input_tokens, Some(0));
        assert!(
            !serde_json::to_string(&observations[0].usage)
                .unwrap()
                .contains("NATIVE_PRIVATE_CANARY")
        );
        assert!(reader.poll().is_empty());
        database.execute("UPDATE message SET data=json_set(data,'$.time.completed',4) WHERE id='new-pending'", []).unwrap();
        assert_eq!(reader.poll().len(), 2);
        let mut next =
            OpenCodeUsageReader::baseline(path.clone(), "/workspace".into(), "session-1".into())
                .unwrap();
        insert("next-call", "session-1", Some(5));
        assert_eq!(next.poll().len(), 2);
        for (source_index, source) in [
            include_str!("../../../docs/research/runtime-monitoring/fixtures/round5-native-usage-context.json"),
            include_str!("../../../docs/research/runtime-monitoring/fixtures/round6-native-context-ratio.json"),
            include_str!("../../../docs/research/runtime-monitoring/fixtures/round7-native-boundaries.json"),
        ].into_iter().enumerate() {
        let witness: serde_json::Value = serde_json::from_str(source).unwrap();
        let entry = witness["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["runtime"] == "opencode-cli")
            .unwrap();
        for (run_index, run) in entry["runs"].as_array().unwrap().iter().enumerate() {
            let mut replay = OpenCodeUsageReader::baseline(
                path.clone(),
                "/workspace".into(),
                "session-1".into(),
            )
            .unwrap();
            for (call_index, record) in run["sourceRecords"].as_array().unwrap().iter().enumerate()
            {
                database
                    .execute(
                        "INSERT INTO message VALUES(?1,'session-1',1,?2)",
                        rusqlite::params![
                            format!("witness-{source_index}-{run_index}-{call_index}"),
                            record["raw"].to_string()
                        ],
                    )
                    .unwrap();
                let observations = replay.poll();
                assert_eq!(observations.len(), 2);
                let fields = &observations[0].usage.fields;
                let total = fields.input_tokens.and_then(|input| {
                    Some(input + fields.cache_read_input_tokens? + fields.cache_write_input_tokens?)
                });
                assert_eq!(
                    json!({
                        "promptInputTotalTokens":total,"outputTokens":fields.output_tokens,
                        "cacheReadTokens":fields.cache_read_input_tokens,"cacheWriteTokens":fields.cache_write_input_tokens
                    }),
                    record["expectedParsed"]
                );
                if let Some(context) = record.get("expectedContext") {
                    assert_eq!(
                        json!({"usedTokens":observations[1].usage.fields.context_used_tokens,
                            "windowTokens":observations[1].usage.fields.context_size_tokens,
                            "nativeRatio":observations[1].usage.fields.native_context_ratio}),
                        *context
                    );
                }
                assert!(replay.poll().is_empty());
            }
        }
        }
        database
            .execute(
                "UPDATE session SET directory='/changed' WHERE id='session-1'",
                [],
            )
            .unwrap();
        assert!(next.poll().is_empty());
        assert!(next.disabled);
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }
}
