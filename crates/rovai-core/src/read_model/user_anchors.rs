//! Complete public-user navigation, independent of the message/Run display windows.
//! Reads only navigation text; it never hydrates messages, evidence or attachment bytes.
use super::*;
use crate::camp_content::{member_mention_ids, render_plain_text_with_current_user};
use crate::current_user::CurrentUserResolver;

const SUMMARY_SCALARS: usize = 240;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUserAnchor {
    pub message_id: String,
    pub sequence: i64,
    pub title: String,
    pub message_version: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUserAnchorIndex {
    pub schema_version: i64,
    #[serde(rename = "threadId")]
    pub camp_id: String,
    pub through_global_sequence: i64,
    pub total_count: usize,
    pub items: Vec<ThreadUserAnchor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUserAnchorReply {
    pub message_id: String,
    pub sequence: i64,
    pub summary: String,
    pub message_version: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUserAnchorPreview {
    pub schema_version: i64,
    #[serde(rename = "threadId")]
    pub camp_id: String,
    pub message_id: String,
    pub through_global_sequence: i64,
    pub source_available: bool,
    pub first_reply: Option<ThreadUserAnchorReply>,
}

const NAVIGABLE_USER: &str = "message.author_type IN ('user', 'external_principal')
    AND message.tombstoned_at IS NULL AND message.recall_state <> 'withdrawn'
    AND NOT EXISTS (SELECT 1 FROM mission_start WHERE message_id = message.id)";

impl ReadModelService {
    pub fn user_anchors(
        &self,
        database: &mut Database,
        camp_id: &str,
    ) -> Result<ThreadUserAnchorIndex> {
        let tx = database.connection_mut().transaction()?;
        load_camp(&tx, camp_id)?.context("Thread does not exist")?;
        let through_global_sequence = current_global_sequence(&tx)?;
        let sql = format!(
            "SELECT message.id, message.sequence, message.version,
            message.structured_content_json, message.source_attachments_json, message.quotes_json
            FROM camp_message AS message WHERE message.camp_id = ?1 AND {NAVIGABLE_USER}
            ORDER BY message.sequence, message.id"
        );
        let rows = tx
            .prepare(&sql)?
            .query_map([camp_id], text_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let items = navigation_text(&tx, rows)?
            .into_iter()
            .map(|(row, title)| ThreadUserAnchor {
                message_id: row.id,
                sequence: row.sequence,
                message_version: row.version,
                title,
            })
            .collect::<Vec<_>>();
        tx.commit()?;
        Ok(ThreadUserAnchorIndex {
            schema_version: 1,
            camp_id: camp_id.into(),
            through_global_sequence,
            total_count: items.len(),
            items,
        })
    }

    pub fn user_anchor_preview(
        &self,
        database: &mut Database,
        camp_id: &str,
        message_id: &str,
    ) -> Result<ThreadUserAnchorPreview> {
        let tx = database.connection_mut().transaction()?;
        load_camp(&tx, camp_id)?.context("Thread does not exist")?;
        let through_global_sequence = current_global_sequence(&tx)?;
        let sequence: Option<i64> = tx
            .query_row(
                &format!(
                    "SELECT message.sequence FROM camp_message AS message
             WHERE message.camp_id = ?1 AND message.id = ?2 AND {NAVIGABLE_USER}"
                ),
                params![camp_id, message_id],
                |row| row.get(0),
            )
            .optional()?;
        // Explicit replies take precedence, even when they address another message.
        // An existing Run's complete input set takes precedence over the message's Turn.
        // The candidate pass reads identities and relationships, never reply bodies.
        let reply_id = if let Some(sequence) = sequence {
            tx.query_row(r#"
                SELECT reply.id
                FROM camp_message AS reply
                LEFT JOIN agent_run AS run ON run.id = reply.source_agent_run_id
                  AND run.invocation_kind <> 'single_chat'
                  AND COALESCE(run.camp_id, (SELECT camp_id FROM camp_turn WHERE id = run.camp_turn_id)) = ?1
                WHERE reply.camp_id = ?1 AND reply.author_type = 'agent'
                  AND reply.sequence > ?3 AND reply.tombstoned_at IS NULL
                  AND reply.recall_state <> 'withdrawn'
                  AND NOT EXISTS (SELECT 1 FROM mission_start WHERE message_id = reply.id)
                  AND CASE
                    WHEN reply.reply_to_camp_message_id IS NOT NULL THEN reply.reply_to_camp_message_id = ?2
                    WHEN run.id IS NOT NULL THEN
                      run.anchor_message_id = ?2
                      OR EXISTS (SELECT 1 FROM agent_run_input WHERE agent_run_id = run.id AND message_id = ?2)
                      OR EXISTS (SELECT 1 FROM camp_turn WHERE id = run.camp_turn_id AND camp_id = ?1
                                 AND trigger_type = 'camp_message' AND trigger_id = ?2)
                    ELSE EXISTS (SELECT 1 FROM camp_turn WHERE id = reply.camp_turn_id AND camp_id = ?1
                                 AND trigger_type = 'camp_message' AND trigger_id = ?2)
                  END
                ORDER BY reply.sequence, reply.id LIMIT 1
            "#, params![camp_id, message_id, sequence], |row| row.get::<_, String>(0)).optional()?
        } else {
            None
        };
        let first_reply = if let Some(id) = reply_id {
            let row = tx.query_row("SELECT id, sequence, version, structured_content_json,
                source_attachments_json, quotes_json FROM camp_message WHERE id = ?1 AND camp_id = ?2",
                params![id, camp_id], text_row)?;
            navigation_text(&tx, vec![row])?
                .into_iter()
                .next()
                .map(|(row, summary)| ThreadUserAnchorReply {
                    message_id: row.id,
                    sequence: row.sequence,
                    message_version: row.version,
                    summary,
                })
        } else {
            None
        };
        tx.commit()?;
        Ok(ThreadUserAnchorPreview {
            schema_version: 1,
            camp_id: camp_id.into(),
            message_id: message_id.into(),
            through_global_sequence,
            source_available: sequence.is_some(),
            first_reply,
        })
    }
}

struct NavigationText {
    id: String,
    sequence: i64,
    version: i64,
    content: String,
    sources: String,
    quotes: String,
}

fn text_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NavigationText> {
    Ok(NavigationText {
        id: row.get(0)?,
        sequence: row.get(1)?,
        version: row.get(2)?,
        content: row.get(3)?,
        sources: row.get(4)?,
        quotes: row.get(5)?,
    })
}

fn navigation_text(
    tx: &Transaction<'_>,
    rows: Vec<NavigationText>,
) -> Result<Vec<(NavigationText, String)>> {
    let contents = rows
        .iter()
        .map(|row| {
            serde_json::from_str::<StructuredThreadMessageContent>(&row.content)
                .map(normalize_content)
        })
        .collect::<serde_json::Result<Vec<_>>>()?;
    let mentions = contents
        .iter()
        .flat_map(|content| member_mention_ids(content))
        .collect::<BTreeSet<_>>();
    let mut names = BTreeMap::new();
    let mut query = tx.prepare(
        "SELECT id, display_name FROM agent_profile
        WHERE id IN (SELECT value FROM json_each(?1))",
    )?;
    for name in query.query_map([serde_json::to_string(&mentions)?], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, name) = name?;
        names.insert(id, name);
    }
    let current_user = CurrentUserResolver::resolve("zh-CN");
    let mut texts = contents
        .iter()
        .map(|content| {
            render_plain_text_with_current_user(
                content,
                |id| names.get(id).cloned(),
                current_user.display_name,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let empty = rows
        .iter()
        .zip(&texts)
        .filter(|(_, text)| text.trim().is_empty())
        .map(|(row, _)| row.id.as_str())
        .collect::<Vec<_>>();
    let mut attachments = BTreeMap::<String, Vec<String>>::new();
    if !empty.is_empty() {
        // Both historical attachment representations are metadata-only and fetched once.
        let mut query = tx.prepare(r#"
            WITH requested AS (SELECT value AS id FROM json_each(?1))
            SELECT message_id, name FROM (
                SELECT a.camp_message_id AS message_id, a.display_name AS name, a.position AS ordinal, a.id
                FROM requested JOIN message_attachment a ON a.camp_message_id = requested.id
                UNION ALL
                SELECT a.camp_message_id, a.display_name_snapshot, a.ordinal, a.attachment_id
                FROM requested JOIN camp_message_attachment_ref a ON a.camp_message_id = requested.id
            ) ORDER BY message_id, ordinal, id
        "#)?;
        for row in query.query_map([serde_json::to_string(&empty)?], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (id, name) = row?;
            attachments.entry(id).or_default().push(name);
        }
    }
    rows.into_iter()
        .zip(texts.drain(..))
        .map(|(row, mut text)| {
            if text.trim().is_empty() {
                let mut names = parse_source_attachments(&row.sources)?
                    .into_iter()
                    .map(|a| a.display_name)
                    .collect::<Vec<_>>();
                names.extend(attachments.remove(&row.id).unwrap_or_default());
                text = names.join("、");
            }
            if text.trim().is_empty() {
                text = serde_json::from_str::<Vec<MessageQuoteSnapshot>>(&row.quotes)?
                    .into_iter()
                    .map(|quote| quote.text)
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            if text.trim().is_empty() {
                text = "（无文本）".into();
            }
            // Same scalar budget as the existing Run input summary; CSS owns visible length.
            let mut chars = text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .collect::<Vec<_>>();
            if chars.len() > SUMMARY_SCALARS {
                chars.truncate(SUMMARY_SCALARS);
                chars[SUMMARY_SCALARS - 1] = '…';
            }
            Ok((row, chars.into_iter().collect()))
        })
        .collect()
}
