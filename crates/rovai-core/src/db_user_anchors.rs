//! Direct-reply lookup needs equality on Thread/reply before reading sequence order.
use super::*;

const INDEX: &str = "CREATE INDEX camp_message_direct_reply_idx ON camp_message(camp_id, reply_to_camp_message_id, sequence, id) WHERE reply_to_camp_message_id IS NOT NULL";

pub(super) fn schema_matches(connection: &Connection) -> rusqlite::Result<bool> {
    let sql: Option<String> = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='index' AND name='camp_message_direct_reply_idx'",
        [], |row| row.get(0)).optional()?;
    Ok(sql.as_deref() == Some(INDEX))
}

pub(super) fn migrate(database: &mut Database) -> Result<()> {
    let tx = database
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    anyhow::ensure!(
        matches!(classify_database_contract(&tx)?,
        DatabaseContractClassification::SupportedMigrationSource(ref marker)
        if marker.contract_version == "v1.72" && marker.projection_schema_version == 135),
        "Direct reply index requires v1.72/schema 135"
    );
    tx.execute_batch(INDEX)?;
    tx.execute_batch("INSERT INTO schema_migration VALUES(186,datetime('now'));
        UPDATE rovai_data_contract SET projection_schema_version=136,updated_at=datetime('now') WHERE singleton=1;")?;
    anyhow::ensure!(
        matches!(
            classify_database_contract(&tx)?,
            DatabaseContractClassification::SupportedMigrationSource(ref marker) if marker.projection_schema_version == 136
        ),
        "Direct reply index admission failed"
    );
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
pub(super) fn downgrade_for_test(connection: &Connection) {
    message_mentions::downgrade_for_test(connection);
    connection.execute_batch("DROP INDEX IF EXISTS camp_message_direct_reply_idx;
        DELETE FROM schema_migration WHERE version=186;
        UPDATE rovai_data_contract SET projection_schema_version=135 WHERE singleton=1 AND projection_schema_version=136;").unwrap();
}

#[cfg(all(test, feature = "extended-tests"))]
mod tests {
    use super::*;

    // Owns the new schema 135 -> 136 admission/DDL atomicity; query tests cannot
    // prove an installed database survives a failed receipt and an exact reopen.
    #[test]
    fn direct_reply_index_migration_is_atomic() {
        let (mut database, directory) = crate::test_support::seeded_runtime_database();
        downgrade_for_test(database.connection());
        let before = public_history_claim_preserved_evidence_digest(database.connection()).unwrap();
        database
            .connection()
            .execute_batch(
                "CREATE TRIGGER reject_anchor_receipt BEFORE INSERT ON schema_migration
            WHEN NEW.version=186 BEGIN SELECT RAISE(ABORT,'fixture failure'); END;",
            )
            .unwrap();
        assert!(migrate(&mut database).is_err());
        assert!(!schema_matches(database.connection()).unwrap());
        assert!(!database.schema_migration_applied(186).unwrap());
        assert!(
            matches!(classify_database_contract(database.connection()).unwrap(),
            DatabaseContractClassification::SupportedMigrationSource(ref marker) if marker.projection_schema_version == 135)
        );
        database
            .connection()
            .execute_batch("DROP TRIGGER reject_anchor_receipt")
            .unwrap();
        migrate(&mut database).unwrap();
        message_mentions::migrate(&mut database).unwrap();
        assert_eq!(
            public_history_claim_preserved_evidence_digest(database.connection()).unwrap(),
            before
        );
        drop(database);
        let database = Database::open(&directory).unwrap();
        assert!(schema_matches(database.connection()).unwrap());
        assert!(matches!(
            classify_database_contract(database.connection()).unwrap(),
            DatabaseContractClassification::Current(_)
        ));
        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
