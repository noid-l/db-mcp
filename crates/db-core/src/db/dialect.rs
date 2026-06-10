use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use sqlx::any::Any;
use std::collections::HashMap;
use crate::db::{ColumnMetadata, TableMetadata};

#[async_trait]
pub trait DbDialect: Send + Sync {
    #[allow(dead_code)]
    fn db_type(&self) -> &str;

    fn format_table_name(&self, database: &str, table: &str) -> String {
        format!("\"{}\".\"{}\"", database, table)
    }

    fn apply_limit(&self, sql: &str, _limit: usize) -> String {
        sql.to_string()
    }

    fn get_default_db_query(&self) -> Option<&str>;

    async fn list_databases(&self, pool: &sqlx::Pool<Any>) -> Result<Vec<String>>;

    async fn list_tables(&self, pool: &sqlx::Pool<Any>, database: &str) -> Result<Vec<TableMetadata>>;

    async fn describe_table(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<ColumnMetadata>>;

    async fn list_indexes(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>>;

    async fn get_imported_keys(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>>;

    async fn get_schema_fingerprint(&self, _pool: &sqlx::Pool<Any>, _database: &str) -> Result<Option<String>> {
        Ok(None)
    }

    async fn get_table_ddl(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<String> {
        let cols = self.describe_table(pool, database, table).await?;
        let mut ddl = format!("CREATE TABLE \"{}\".\"{}\" (\n", database, table);
        let mut pk_cols = Vec::new();
        for (i, col) in cols.iter().enumerate() {
            let nullable_str = if col.nullable { " NULL" } else { " NOT NULL" };
            let comma = if i == cols.len() - 1 && pk_cols.is_empty() {
                ""
            } else {
                ","
            };
            ddl.push_str(&format!(
                "  \"{}\" {}{}{}\n",
                col.name, col.type_name, nullable_str, comma
            ));
            if col.primary_key {
                pk_cols.push(format!("\"{}\"", col.name));
            }
        }
        if !pk_cols.is_empty() {
            ddl.push_str(&format!("  PRIMARY KEY ({})\n", pk_cols.join(", ")));
        }
        ddl.push_str(");");
        Ok(ddl)
    }
}

pub fn get_dialect(db_type: &str) -> Result<Box<dyn DbDialect>> {
    let db_type_upper = db_type.to_uppercase();
    match db_type_upper.as_str() {
        "MYSQL" | "MARIADB" | "GBASE" => Ok(Box::new(crate::db::mysql::MySqlDialect)),
        "POSTGRESQL" | "KINGBASE" | "HIGHGO" => Ok(Box::new(crate::db::postgres::PostgreSqlDialect)),
        "SQLITE" => Ok(Box::new(crate::db::sqlite::SqliteDialect)),
        "SQLSERVER" => Ok(Box::new(crate::db::sqlserver::SqlServerDialect)),
        _ => Err(anyhow::anyhow!("Unsupported database type: {}", db_type)),
    }
}
