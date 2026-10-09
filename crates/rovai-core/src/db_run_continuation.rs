//! A continuation is an authorized input source on the existing Delivery, not a second queue.
use super::*;

const MESSAGE_REFERENCE: &str =
    "message_id TEXT NOT NULL REFERENCES camp_message(id) ON DELETE CASCADE,";
const REQUEST_REFERENCE: &str = "message_id TEXT REFERENCES camp_message(id) ON DELETE CASCADE,
                    source_kind TEXT NOT NULL DEFAULT 'message' CHECK(
                        (source_kind = 'message' AND message_id IS NOT NULL)
                        OR (source_kind = 'continuation' AND message_id IS NULL)),";
const REQUEST_GUARDS: &[(&str, &str)] = &[
    (
        "camp_delivery_source_immutable",
        "CREATE TRIGGER camp_delivery_source_immutable
        BEFORE UPDATE OF source_kind,message_id ON camp_message_delivery
        WHEN NEW.source_kind IS NOT OLD.source_kind OR NEW.message_id IS NOT OLD.message_id
        BEGIN SELECT RAISE(ABORT, 'Delivery source is immutable'); END",
    ),
    (
        "camp_continuation_source_kind",
        "CREATE TRIGGER camp_continuation_source_kind
        BEFORE INSERT ON camp_run_continuation
        WHEN NOT EXISTS(SELECT 1 FROM camp_message_delivery WHERE id=NEW.delivery_id
            AND source_kind='continuation' AND message_id IS NULL)
        BEGIN SELECT RAISE(ABORT, 'Continuation requires an internal Delivery'); END",
    ),
];

pub(super) fn request_schema_matches(connection: &Connection) -> rusqlite::Result<bool> {
    let shape: bool = connection.query_row("SELECT
        EXISTS(SELECT 1 FROM pragma_table_info('camp') WHERE name='last_delivery_sequence'
            AND type='INTEGER' AND [notnull]=1 AND dflt_value='0')
        AND EXISTS(SELECT 1 FROM pragma_table_info('camp_message_delivery') WHERE name='message_id' AND [notnull]=0)
        AND EXISTS(SELECT 1 FROM sqlite_master WHERE name='camp_message_delivery' AND instr(sql,?1)>0)",
        [REQUEST_REFERENCE], |row| row.get(0))?;
    if !shape {
        return Ok(false);
    }
    for (name, sql) in REQUEST_GUARDS {
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        let normalize = |value: &str| value.split_whitespace().collect::<String>();
        if actual.as_deref().map(normalize) != Some(normalize(sql)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Changes only the queue's source model. Published messages and frozen evidence
/// are retained, including the old operation records from development builds.
pub(super) fn migrate_requests(database: &mut Database) -> Result<()> {
    database
        .connection
        .execute_batch("PRAGMA foreign_keys=OFF;")?;
    let result = (|| -> Result<()> {
        let tx = database
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        anyhow::ensure!(
            matches!(classify_database_contract(&tx)?,
            DatabaseContractClassification::SupportedMigrationSource(ref marker)
                if marker.contract_version=="v1.72" && marker.projection_schema_version==137),
            "Continuation requests require v1.72/schema 137"
        );
        let before = public_history_claim_preserved_evidence_digest(&tx)?;
        let source: String = tx.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='camp_message_delivery'",
            [],
            |r| r.get(0),
        )?;
        anyhow::ensure!(
            source.contains(MESSAGE_REFERENCE),
            "Missing message Delivery source constraint"
        );
        let target = replacement_table_schema_v171(source, "camp_message_delivery")
            .replace(MESSAGE_REFERENCE, REQUEST_REFERENCE);
        rebuild_table_v171(&tx, "camp_message_delivery", &target, &[], &[])?;
        tx.execute_batch("ALTER TABLE camp ADD COLUMN last_delivery_sequence INTEGER NOT NULL DEFAULT 0 CHECK(last_delivery_sequence>=0);
            UPDATE camp SET last_delivery_sequence=COALESCE((SELECT MAX(queue_sequence) FROM camp_message_delivery WHERE camp_id=camp.id),0);
            UPDATE camp_message_delivery SET message_id=NULL,source_kind='continuation'
                WHERE EXISTS(SELECT 1 FROM camp_run_continuation WHERE delivery_id=camp_message_delivery.id);")?;
        for (_, sql) in REQUEST_GUARDS {
            tx.execute_batch(sql)?;
        }
        tx.execute_batch("INSERT INTO schema_migration VALUES(188,datetime('now'));
            UPDATE rovai_data_contract SET projection_schema_version=138,updated_at=datetime('now') WHERE singleton=1;")?;
        anyhow::ensure!(
            public_history_claim_preserved_evidence_digest(&tx)? == before,
            "Continuation request migration changed frozen evidence"
        );
        validate_migration_foreign_keys(
            &tx,
            &[
                "camp_message_delivery",
                "camp_run_continuation",
                "agent_run_input",
            ],
        )?;
        anyhow::ensure!(
            matches!(
                classify_database_contract(&tx)?,
                DatabaseContractClassification::Current(_)
            ),
            "Continuation request schema admission failed"
        );
        tx.commit()?;
        Ok(())
    })();
    let restored = database.connection.execute_batch("PRAGMA foreign_keys=ON;");
    result?;
    restored?;
    Ok(())
}

#[cfg(test)]
pub(super) fn downgrade_requests_for_test(connection: &Connection) {
    if !connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migration WHERE version=188)",
            [],
            |r| r.get::<_, bool>(0),
        )
        .unwrap()
    {
        return;
    }
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    let tx = connection.unchecked_transaction().unwrap();
    tx.execute_batch(
        "DROP TRIGGER camp_delivery_source_immutable; DROP TRIGGER camp_continuation_source_kind;",
    )
    .unwrap();
    let source: String = tx
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='camp_message_delivery'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let target = replacement_table_schema_v171(source, "camp_message_delivery")
        .replace(REQUEST_REFERENCE, MESSAGE_REFERENCE);
    rebuild_table_v171(&tx, "camp_message_delivery", &target, &["source_kind"], &[]).unwrap();
    tx.execute_batch(
        "ALTER TABLE camp DROP COLUMN last_delivery_sequence;
        DELETE FROM schema_migration WHERE version=188;
        UPDATE rovai_data_contract SET projection_schema_version=137 WHERE singleton=1;",
    )
    .unwrap();
    tx.commit().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
}

const OBJECTS: &[(&str, &str)] = &[
    ("camp_run_continuation", "CREATE TABLE camp_run_continuation (
        delivery_id TEXT PRIMARY KEY REFERENCES camp_message_delivery(id) ON DELETE CASCADE,
        source_agent_run_id TEXT NOT NULL REFERENCES agent_run(id),
        use_new_session INTEGER NOT NULL CHECK(use_new_session IN (0,1))
    )"),
    ("camp_run_continuation_immutable", "CREATE TRIGGER camp_run_continuation_immutable
        BEFORE UPDATE ON camp_run_continuation BEGIN SELECT RAISE(ABORT, 'continuation authorization is immutable'); END"),
    ("agent_run_input_delivery_idx", "CREATE INDEX agent_run_input_delivery_idx ON agent_run_input(delivery_id)"),
    ("agent_run_input_delivery_owner", "CREATE TRIGGER agent_run_input_delivery_owner
        BEFORE INSERT ON agent_run_input WHEN EXISTS (
            SELECT 1 FROM agent_run_input AS prior WHERE prior.delivery_id=NEW.delivery_id
              AND (prior.agent_run_id<>NEW.agent_run_id OR NOT EXISTS (
                  SELECT 1 FROM camp_run_continuation WHERE delivery_id=NEW.delivery_id))
        ) BEGIN SELECT RAISE(ABORT, 'Delivery input belongs to one authorized Run'); END"),
];

pub(super) fn schema_matches(connection: &Connection) -> rusqlite::Result<bool> {
    for (name, expected) in OBJECTS {
        let actual: Option<String> = connection
            .query_row("SELECT sql FROM sqlite_master WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .optional()?;
        let normalize = |sql: &str| sql.split_whitespace().collect::<String>();
        if actual.as_deref().map(normalize) != Some(normalize(expected)) {
            return Ok(false);
        }
    }
    let input: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_run_input'",
        [],
        |r| r.get(0),
    )?;
    Ok(!input.contains("delivery_id TEXT NOT NULL UNIQUE"))
}

pub(super) fn migrate(database: &mut Database) -> Result<()> {
    database
        .connection
        .execute_batch("PRAGMA foreign_keys=OFF;")?;
    let result = (|| -> Result<()> {
        let tx = database
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        anyhow::ensure!(
            matches!(classify_database_contract(&tx)?,DatabaseContractClassification::SupportedMigrationSource(ref marker)
            if marker.contract_version=="v1.72" && marker.projection_schema_version==133),
            "Run continuation requires v1.72/schema 133"
        );
        let before = public_history_claim_preserved_evidence_digest(&tx)?;
        let source: String = tx.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_run_input'",
            [],
            |r| r.get(0),
        )?;
        anyhow::ensure!(
            source.contains("delivery_id TEXT NOT NULL UNIQUE"),
            "Missing original Delivery uniqueness constraint"
        );
        let target = replacement_table_schema_v171(source, "agent_run_input").replace(
            "delivery_id TEXT NOT NULL UNIQUE",
            "delivery_id TEXT NOT NULL",
        );
        rebuild_table_v171(&tx, "agent_run_input", &target, &[], &[])?;
        for (_, sql) in OBJECTS {
            tx.execute_batch(sql)?;
        }
        tx.execute_batch("INSERT INTO schema_migration VALUES(184,datetime('now'));
            UPDATE rovai_data_contract SET projection_schema_version=134,updated_at=datetime('now') WHERE singleton=1;")?;
        anyhow::ensure!(
            before == public_history_claim_preserved_evidence_digest(&tx)?,
            "Continuation migration changed existing model evidence"
        );
        validate_migration_foreign_keys(&tx, &["agent_run_input", "camp_run_continuation"])?;
        anyhow::ensure!(
            matches!(
                classify_database_contract(&tx)?,
                DatabaseContractClassification::SupportedMigrationSource(ref marker)
                    if marker.projection_schema_version == 134
            ),
            "Continuation schema admission failed"
        );
        tx.commit()?;
        Ok(())
    })();
    let restored = database.connection.execute_batch("PRAGMA foreign_keys=ON;");
    result?;
    restored?;
    Ok(())
}

#[cfg(test)]
pub(super) fn downgrade_for_test(connection: &Connection) {
    mission_description::downgrade_for_test(connection);
    if !connection
        .table_exists(None, "camp_run_continuation")
        .unwrap()
    {
        return;
    }
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    let tx = connection.unchecked_transaction().unwrap();
    tx.execute_batch(
        "DROP TRIGGER camp_run_continuation_immutable; DROP TRIGGER agent_run_input_delivery_owner;
        DROP INDEX agent_run_input_delivery_idx; DROP TABLE camp_run_continuation;",
    )
    .unwrap();
    let source: String = tx
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_run_input'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let target = replacement_table_schema_v171(source, "agent_run_input").replace(
        "delivery_id TEXT NOT NULL",
        "delivery_id TEXT NOT NULL UNIQUE",
    );
    rebuild_table_v171(&tx, "agent_run_input", &target, &[], &[]).unwrap();
    tx.execute_batch("DELETE FROM schema_migration WHERE version=184; UPDATE rovai_data_contract SET projection_schema_version=133 WHERE singleton=1;").unwrap();
    tx.commit().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
}

// Reuse the populated queue owner to exercise the legacy source conversion.
#[cfg(all(test, feature = "extended-tests"))]
pub(crate) fn assert_legacy_request_upgrade(database: &mut Database, sources: &[(&str, &str)]) {
    let connection = database.connection();
    connection
        .execute_batch("DROP TRIGGER camp_delivery_source_immutable")
        .unwrap();
    for (delivery, message) in sources {
        connection
            .execute(
                "UPDATE camp_message_delivery SET source_kind='message',message_id=?2 WHERE id=?1",
                params![delivery, message],
            )
            .unwrap();
    }
    connection.execute_batch(REQUEST_GUARDS[0].1).unwrap();
    downgrade_requests_for_test(connection);
    let before = public_history_claim_preserved_evidence_digest(connection).unwrap();
    let messages = |connection: &Connection| -> Vec<String> {
        connection.prepare("SELECT json_object('id',id,'body',body,'content',structured_content_json,'sequence',sequence,'tombstone',tombstoned_at,'version',version) FROM camp_message ORDER BY id")
            .unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    };
    let old_messages = messages(connection);
    let public_tail: i64 = connection
        .query_row("SELECT SUM(last_message_sequence) FROM camp", [], |r| {
            r.get(0)
        })
        .unwrap();
    migrate_requests(database).unwrap();
    let connection = database.connection();
    assert_eq!(messages(connection), old_messages);
    assert_eq!(
        public_history_claim_preserved_evidence_digest(connection).unwrap(),
        before
    );
    assert_eq!(
        connection
            .query_row("SELECT SUM(last_message_sequence) FROM camp", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        public_tail
    );
    for (delivery, _) in sources {
        assert!(connection.query_row("SELECT source_kind='continuation' AND message_id IS NULL FROM camp_message_delivery WHERE id=?1",[delivery],|r|r.get::<_,bool>(0)).unwrap());
    }
    assert!(connection.query_row("SELECT NOT EXISTS(SELECT 1 FROM camp WHERE last_delivery_sequence<>COALESCE((SELECT MAX(queue_sequence) FROM camp_message_delivery WHERE camp_id=camp.id),0))",[],|r|r.get::<_,bool>(0)).unwrap());
}

#[cfg(all(test, feature = "extended-tests"))]
mod tests {
    use super::*;

    // Owns continuation schema changes (133 -> 134 and 137 -> 138): a failure after
    // rebuilding the table must restore both evidence and schema atomically.
    #[test]
    fn continuation_migration_rolls_back_and_preserves_frozen_evidence() {
        let (mut database, directory) = crate::test_support::seeded_runtime_database();
        downgrade_requests_for_test(database.connection());
        let request_schema: String = database
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='camp_message_delivery'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        database.connection().execute_batch("CREATE TRIGGER reject_request_receipt BEFORE INSERT ON schema_migration WHEN NEW.version=188 BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(migrate_requests(&mut database).is_err());
        assert!(!database.schema_migration_applied(188).unwrap());
        assert_eq!(
            database
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name='camp_message_delivery'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            request_schema
        );
        assert_eq!(
            database
                .connection()
                .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        database
            .connection()
            .execute_batch("DROP TRIGGER reject_request_receipt")
            .unwrap();
        migrate_requests(&mut database).unwrap();
        assert!(request_schema_matches(database.connection()).unwrap());
        downgrade_for_test(database.connection());
        let before = public_history_claim_preserved_evidence_digest(database.connection()).unwrap();
        let schema: String = database
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='agent_run_input'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        database.connection().execute_batch("CREATE TRIGGER reject_continuation_receipt BEFORE INSERT ON schema_migration WHEN NEW.version=184 BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(migrate(&mut database).is_err());
        assert!(!database.schema_migration_applied(184).unwrap());
        assert!(
            !database
                .connection()
                .table_exists(None, "camp_run_continuation")
                .unwrap()
        );
        assert_eq!(
            database
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name='agent_run_input'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            schema
        );
        assert_eq!(
            database
                .connection()
                .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            public_history_claim_preserved_evidence_digest(database.connection()).unwrap(),
            before
        );
        database
            .connection()
            .execute_batch("DROP TRIGGER reject_continuation_receipt")
            .unwrap();
        migrate(&mut database).unwrap();
        assert!(schema_matches(database.connection()).unwrap());
        drop(database);
        let database = Database::open(&directory).unwrap();
        assert_eq!(
            public_history_claim_preserved_evidence_digest(database.connection()).unwrap(),
            before
        );
        assert!(matches!(
            classify_database_contract(database.connection()).unwrap(),
            DatabaseContractClassification::Current(_)
        ));
        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
