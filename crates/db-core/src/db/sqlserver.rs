use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use sqlx::{Row, any::Any};
use std::collections::HashMap;
use crate::db::{ColumnMetadata, TableMetadata, DbDialect};

pub struct SqlServerDialect;

#[async_trait]
impl DbDialect for SqlServerDialect {
    fn db_type(&self) -> &str {
        "SQLSERVER"
    }

    fn get_default_db_query(&self) -> Option<&str> {
        Some("SELECT DB_NAME()")
    }

    async fn list_databases(&self, pool: &sqlx::Pool<Any>) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT name FROM sys.databases WHERE name NOT IN ('master', 'tempdb', 'model', 'msdb')").fetch_all(pool).await?;
        let mut dbs = Vec::new();
        for row in rows {
            if let Ok(name) = row.try_get::<String, _>(0) {
                dbs.push(name);
            }
        }
        Ok(dbs)
    }

    async fn list_tables(&self, pool: &sqlx::Pool<Any>, database: &str) -> Result<Vec<TableMetadata>> {
        let rows = sqlx::query("SELECT table_name, table_type FROM information_schema.tables WHERE table_schema = @p1")
            .bind(database)
            .fetch_all(pool).await?;
        let mut tables = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let t_type: String = row.try_get(1)?;
            tables.push(TableMetadata {
                name,
                catalog: None,
                schema: Some(database.to_string()),
                table_type: t_type,
                comment: "".to_string(),
            });
        }
        Ok(tables)
    }

    async fn describe_table(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<ColumnMetadata>> {
        let rows = sqlx::query("SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, CASE WHEN k.column_name IS NOT NULL THEN 1 ELSE 0 END AS is_pk FROM information_schema.columns c LEFT JOIN (SELECT ku.table_catalog, ku.table_schema, ku.table_name, ku.column_name FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage ku ON tc.constraint_name = ku.constraint_name WHERE tc.constraint_type = 'PRIMARY KEY') k ON c.table_catalog = k.table_catalog AND c.table_schema = k.table_schema AND c.table_name = k.table_name AND c.column_name = k.column_name WHERE c.table_schema = @p1 AND c.table_name = @p2 ORDER BY c.ordinal_position")
            .bind(database)
            .bind(table)
            .fetch_all(pool).await?;
        let mut columns = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let type_name: String = row.try_get(1)?;
            let is_nullable: String = row.try_get(2)?;
            let def_val: Option<String> = row.try_get(3).ok();
            let is_pk: i32 = row.try_get(4)?;
            columns.push(ColumnMetadata {
                name,
                sql_type: 0,
                type_name,
                size: 0,
                digits: None,
                nullable: is_nullable.to_lowercase() == "yes",
                default_value: def_val,
                primary_key: is_pk > 0,
                comment: "".to_string(),
            });
        }
        Ok(columns)
    }

    async fn list_indexes(&self, _pool: &sqlx::Pool<Any>, _database: &str, _table: &str) -> Result<Vec<HashMap<String, Value>>> {
        Ok(Vec::new())
    }

    async fn get_imported_keys(&self, _pool: &sqlx::Pool<Any>, _database: &str, _table: &str) -> Result<Vec<HashMap<String, Value>>> {
        Ok(Vec::new())
    }

    async fn get_schema_fingerprint(&self, pool: &sqlx::Pool<Any>, database: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT COUNT(*), SUM(LEN(table_name)) FROM information_schema.tables WHERE table_schema = @p1")
            .bind(database)
            .fetch_one(pool)
            .await?;
        let count: i32 = row.try_get(0)?;
        let len_sum: Option<i32> = row.try_get(1).ok().flatten();
        Ok(Some(format!("{}-{}", count, len_sum.unwrap_or(0))))
    }
}
