use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::{Row, any::Any};
use std::collections::HashMap;
use crate::db::{ColumnMetadata, TableMetadata, DbDialect};

pub struct MySqlDialect;

#[async_trait]
impl DbDialect for MySqlDialect {
    fn db_type(&self) -> &str {
        "MYSQL"
    }

    fn format_table_name(&self, database: &str, table: &str) -> String {
        format!("`{}`.`{}`", database, table)
    }

    fn apply_limit(&self, sql: &str, limit: usize) -> String {
        format!("{} LIMIT {}", sql, limit)
    }

    fn get_default_db_query(&self) -> Option<&str> {
        Some("SELECT DATABASE()")
    }

    async fn list_databases(&self, pool: &sqlx::Pool<Any>) -> Result<Vec<String>> {
        let rows = sqlx::query("SHOW DATABASES").fetch_all(pool).await?;
        let mut dbs = Vec::new();
        for row in rows {
            if let Ok(name) = row.try_get::<String, _>(0) {
                dbs.push(name);
            }
        }
        Ok(dbs)
    }

    async fn list_tables(&self, pool: &sqlx::Pool<Any>, database: &str) -> Result<Vec<TableMetadata>> {
        let rows = sqlx::query("SELECT TABLE_NAME, TABLE_TYPE, COALESCE(TABLE_COMMENT, '') FROM information_schema.TABLES WHERE TABLE_SCHEMA = ?")
            .bind(database)
            .fetch_all(pool).await?;
        let mut tables = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let t_type: String = row.try_get(1)?;
            let comment: String = row.try_get(2)?;
            tables.push(TableMetadata {
                name,
                catalog: None,
                schema: Some(database.to_string()),
                table_type: t_type,
                comment,
            });
        }
        Ok(tables)
    }

    async fn describe_table(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<ColumnMetadata>> {
        let rows = sqlx::query("SELECT COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_COMMENT, CASE WHEN COLUMN_KEY = 'PRI' THEN 1 ELSE 0 END AS IS_PK FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION")
            .bind(database)
            .bind(table)
            .fetch_all(pool).await?;
        let mut columns = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let _data_type: String = row.try_get(1)?;
            let column_type: String = row.try_get(2)?;
            let is_nullable: String = row.try_get(3)?;
            let def_val: Option<String> = row.try_get(4).ok();
            let comment: String = row.try_get(5)?;
            let is_pk: i64 = row.try_get(6)?;
            columns.push(ColumnMetadata {
                name,
                sql_type: 0,
                type_name: column_type,
                size: 0,
                digits: None,
                nullable: is_nullable.to_lowercase() == "yes",
                default_value: def_val,
                primary_key: is_pk > 0,
                comment,
            });
        }
        Ok(columns)
    }

    async fn list_indexes(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>> {
        let rows = sqlx::query("SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, INDEX_TYPE, SEQ_IN_INDEX FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?")
            .bind(database)
            .bind(table)
            .fetch_all(pool).await?;
        let mut indexes = Vec::new();
        for row in rows {
            let idx_name: String = row.try_get(0)?;
            let col_name: String = row.try_get(1)?;
            let non_unique: i64 = row.try_get(2)?;
            let idx_type: String = row.try_get(3)?;
            let seq: i64 = row.try_get(4)?;
            let mut index = HashMap::new();
            index.insert("INDEX_NAME".to_string(), json!(idx_name));
            index.insert("COLUMN_NAME".to_string(), json!(col_name));
            index.insert("NON_UNIQUE".to_string(), json!(non_unique > 0));
            index.insert("TYPE".to_string(), json!(idx_type));
            index.insert("ORDINAL_POSITION".to_string(), json!(seq));
            indexes.push(index);
        }
        Ok(indexes)
    }

    async fn get_imported_keys(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>> {
        let rows = sqlx::query("SELECT REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME, TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION, CONSTRAINT_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND REFERENCED_TABLE_NAME IS NOT NULL")
            .bind(database)
            .bind(table)
            .fetch_all(pool).await?;
        let mut fkeys = Vec::new();
        for row in rows {
            let pk_tbl: String = row.try_get(0)?;
            let pk_col: String = row.try_get(1)?;
            let fk_tbl: String = row.try_get(2)?;
            let fk_col: String = row.try_get(3)?;
            let seq: i64 = row.try_get(4)?;
            let con_name: String = row.try_get(5)?;
            let mut fk = HashMap::new();
            fk.insert("PKTABLE_NAME".to_string(), json!(pk_tbl));
            fk.insert("PKCOLUMN_NAME".to_string(), json!(pk_col));
            fk.insert("FKTABLE_NAME".to_string(), json!(fk_tbl));
            fk.insert("FKCOLUMN_NAME".to_string(), json!(fk_col));
            fk.insert("KEY_SEQ".to_string(), json!(seq));
            fk.insert("FK_NAME".to_string(), json!(con_name));
            fkeys.push(fk);
        }
        Ok(fkeys)
    }

    async fn get_table_ddl(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<String> {
        let q = format!("SHOW CREATE TABLE `{}`.`{}`", database, table);
        if let Ok(row) = sqlx::query(&q).fetch_one(pool).await {
            let sql: String = row.try_get(1)?;
            return Ok(sql);
        }
        let q_view = format!("SHOW CREATE VIEW `{}`.`{}`", database, table);
        let row = sqlx::query(&q_view).fetch_one(pool).await?;
        let sql: String = row.try_get(1)?;
        Ok(sql)
    }

    async fn get_schema_fingerprint(&self, pool: &sqlx::Pool<Any>, database: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT COUNT(*), SUM(LENGTH(TABLE_NAME)) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ?")
            .bind(database)
            .fetch_one(pool)
            .await?;
        let count: i64 = row.try_get(0)?;
        let len_sum: Option<f64> = row.try_get(1).ok().flatten();
        Ok(Some(format!("{}-{}", count, len_sum.unwrap_or(0.0) as i64)))
    }
}
