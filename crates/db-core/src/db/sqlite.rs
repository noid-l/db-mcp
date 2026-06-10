use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::{Row, any::Any};
use std::collections::HashMap;
use crate::db::{ColumnMetadata, TableMetadata, DbDialect};

pub struct SqliteDialect;

#[async_trait]
impl DbDialect for SqliteDialect {
    fn db_type(&self) -> &str {
        "SQLITE"
    }

    fn apply_limit(&self, sql: &str, limit: usize) -> String {
        format!("{} LIMIT {}", sql, limit)
    }

    fn get_default_db_query(&self) -> Option<&str> {
        None
    }

    async fn list_databases(&self, _pool: &sqlx::Pool<Any>) -> Result<Vec<String>> {
        Ok(vec!["main".to_string()])
    }

    async fn list_tables(&self, pool: &sqlx::Pool<Any>, _database: &str) -> Result<Vec<TableMetadata>> {
        let rows = sqlx::query("SELECT name, 'TABLE' as type FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' UNION ALL SELECT name, 'VIEW' as type FROM sqlite_master WHERE type='view'")
            .fetch_all(pool).await?;
        let mut tables = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let t_type: String = row.try_get(1)?;
            tables.push(TableMetadata {
                name,
                catalog: None,
                schema: Some("main".to_string()),
                table_type: t_type,
                comment: "".to_string(),
            });
        }
        Ok(tables)
    }

    async fn describe_table(&self, pool: &sqlx::Pool<Any>, _database: &str, table: &str) -> Result<Vec<ColumnMetadata>> {
        let rows = sqlx::query(&format!("PRAGMA table_info({})", table))
            .fetch_all(pool)
            .await?;
        let mut columns = Vec::new();
        for row in rows {
            let name: String = row.try_get(1)?;
            let type_name: String = row.try_get(2)?;
            let notnull: i64 = row.try_get(3)?;
            let def_val: Option<String> = row.try_get(4).ok();
            let pk: i64 = row.try_get(5)?;
            columns.push(ColumnMetadata {
                name,
                sql_type: 0,
                type_name,
                size: 0,
                digits: None,
                nullable: notnull == 0,
                default_value: def_val,
                primary_key: pk > 0,
                comment: "".to_string(),
            });
        }
        Ok(columns)
    }

    async fn list_indexes(&self, pool: &sqlx::Pool<Any>, _database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>> {
        let rows = sqlx::query(&format!("PRAGMA index_list({})", table))
            .fetch_all(pool)
            .await?;
        let mut items = Vec::new();
        for row in rows {
            let name: String = row.try_get(1)?;
            let unique: i64 = row.try_get(2)?;
            items.push((name, unique > 0));
        }

        let mut indexes = Vec::new();
        for item in items {
            let info_rows = sqlx::query(&format!("PRAGMA index_info({})", item.0))
                .fetch_all(pool)
                .await?;
            for row in info_rows {
                let seqno: i64 = row.try_get(0)?;
                let col_name: String = row.try_get(2)?;
                let mut index = HashMap::new();
                index.insert("INDEX_NAME".to_string(), json!(item.0));
                index.insert("COLUMN_NAME".to_string(), json!(col_name));
                index.insert("NON_UNIQUE".to_string(), json!(!item.1));
                index.insert("ORDINAL_POSITION".to_string(), json!(seqno));
                indexes.push(index);
            }
        }
        Ok(indexes)
    }

    async fn get_imported_keys(&self, pool: &sqlx::Pool<Any>, _database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>> {
        let rows = sqlx::query(&format!("PRAGMA foreign_key_list({})", table))
            .fetch_all(pool)
            .await?;
        let mut fkeys = Vec::new();
        for row in rows {
            let id: i64 = row.try_get(0)?;
            let seq: i64 = row.try_get(1)?;
            let pk_table: String = row.try_get(2)?;
            let fk_col: String = row.try_get(3)?;
            let pk_col: String = row.try_get(4)?;
            let mut fk = HashMap::new();
            fk.insert("PKTABLE_NAME".to_string(), json!(pk_table));
            fk.insert("PKCOLUMN_NAME".to_string(), json!(pk_col));
            fk.insert("FKTABLE_NAME".to_string(), json!(table));
            fk.insert("FKCOLUMN_NAME".to_string(), json!(fk_col));
            fk.insert("KEY_SEQ".to_string(), json!(seq));
            fk.insert("FK_NAME".to_string(), json!(format!("fk_{}_{}", table, id)));
            fkeys.push(fk);
        }
        Ok(fkeys)
    }

    async fn get_table_ddl(&self, pool: &sqlx::Pool<Any>, _database: &str, table: &str) -> Result<String> {
        let row = sqlx::query(
            "SELECT sql FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?",
        )
        .bind(table)
        .fetch_one(pool)
        .await?;
        let sql: String = row.try_get(0)?;
        Ok(sql)
    }

    async fn get_schema_fingerprint(&self, pool: &sqlx::Pool<Any>, _database: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT COUNT(*), SUM(LENGTH(sql)) FROM sqlite_master").fetch_one(pool).await?;
        let count: i64 = row.try_get(0)?;
        let len_sum: Option<i64> = row.try_get(1).ok().flatten();
        Ok(Some(format!("{}-{}", count, len_sum.unwrap_or(0))))
    }
}
