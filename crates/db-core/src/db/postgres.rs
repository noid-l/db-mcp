use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::{Row, any::Any};
use std::collections::HashMap;
use crate::db::{ColumnMetadata, TableMetadata, DbDialect};

pub struct PostgreSqlDialect;

#[async_trait]
impl DbDialect for PostgreSqlDialect {
    fn db_type(&self) -> &str {
        "POSTGRESQL"
    }

    fn apply_limit(&self, sql: &str, limit: usize) -> String {
        format!("{} LIMIT {}", sql, limit)
    }

    fn get_default_db_query(&self) -> Option<&str> {
        Some("SELECT current_database()")
    }

    async fn list_databases(&self, pool: &sqlx::Pool<Any>) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT datname FROM pg_database WHERE datistemplate = false").fetch_all(pool).await?;
        let mut dbs = Vec::new();
        for row in rows {
            if let Ok(name) = row.try_get::<String, _>(0) {
                dbs.push(name);
            }
        }
        Ok(dbs)
    }

    async fn list_tables(&self, pool: &sqlx::Pool<Any>, database: &str) -> Result<Vec<TableMetadata>> {
        let rows = sqlx::query("SELECT c.relname AS table_name, CASE WHEN c.relkind = 'r' THEN 'TABLE' ELSE 'VIEW' END AS table_type, COALESCE(obj_description(c.oid, 'pg_class'), '') AS comment FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relkind IN ('r', 'v')")
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
        let rows = sqlx::query("SELECT a.attname AS column_name, format_type(a.atttypid, a.atttypmod) AS type_name, a.attnotnull = false AS nullable, COALESCE(pg_get_expr(ad.adbin, ad.adrelid), '') AS default_value, COALESCE(i.indisprimary, false) AS is_pk, COALESCE(col_description(a.attrelid, a.attnum), '') AS comment FROM pg_attribute a LEFT JOIN pg_attrdef ad ON a.attrelid = ad.adrelid AND a.attnum = ad.adnum LEFT JOIN pg_index i ON a.attrelid = i.indrelid AND a.attnum = any(i.indkey) JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum")
            .bind(database)
            .bind(table)
            .fetch_all(pool).await?;
        let mut columns = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let type_name: String = row.try_get(1)?;
            let nullable: bool = row.try_get(2)?;
            let def_val: String = row.try_get(3)?;
            let is_pk: bool = row.try_get(4)?;
            let comment: String = row.try_get(5)?;
            columns.push(ColumnMetadata {
                name,
                sql_type: 0,
                type_name,
                size: 0,
                digits: None,
                nullable,
                default_value: if def_val.is_empty() {
                    None
                } else {
                    Some(def_val)
                },
                primary_key: is_pk,
                comment,
            });
        }
        Ok(columns)
    }

    async fn list_indexes(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>> {
        let rows = sqlx::query("SELECT c2.relname AS index_name, i.indisunique = false AS non_unique, pg_get_indexdef(i.indexrelid) AS filter_condition FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid JOIN pg_class c2 ON c2.oid = i.indexrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2")
            .bind(database)
            .bind(table)
            .fetch_all(pool).await?;
        let mut indexes = Vec::new();
        for row in rows {
            let idx_name: String = row.try_get(0)?;
            let non_unique: bool = row.try_get(1)?;
            let filter_condition: String = row.try_get(2)?;
            let mut index = HashMap::new();
            index.insert("INDEX_NAME".to_string(), json!(idx_name));
            index.insert("NON_UNIQUE".to_string(), json!(non_unique));
            index.insert("FILTER_CONDITION".to_string(), json!(filter_condition));
            indexes.push(index);
        }
        Ok(indexes)
    }

    async fn get_imported_keys(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<Vec<HashMap<String, Value>>> {
        let rows = sqlx::query("SELECT conname AS constraint_name, att2.attname AS child_column, cl.relname AS parent_table, att.attname AS parent_column FROM pg_constraint c JOIN pg_class cl ON cl.oid = c.confrelid JOIN pg_attribute att2 ON att2.attrelid = c.conrelid AND att2.attnum = any(c.conkey) JOIN pg_attribute att ON att.attrelid = c.confrelid AND att.attnum = any(c.confkey) JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2 AND c.contype = 'f'")
            .bind(database)
            .bind(table)
            .fetch_all(pool).await?;
        let mut fkeys = Vec::new();
        for (seq, row) in (1..).zip(rows) {
            let con_name: String = row.try_get(0)?;
            let child_col: String = row.try_get(1)?;
            let parent_table: String = row.try_get(2)?;
            let parent_column: String = row.try_get(3)?;
            let mut fk = HashMap::new();
            fk.insert("PKTABLE_NAME".to_string(), json!(parent_table));
            fk.insert("PKCOLUMN_NAME".to_string(), json!(parent_column));
            fk.insert("FKTABLE_NAME".to_string(), json!(table));
            fk.insert("FKCOLUMN_NAME".to_string(), json!(child_col));
            fk.insert("KEY_SEQ".to_string(), json!(seq));
            fk.insert("FK_NAME".to_string(), json!(con_name));
            fkeys.push(fk);
        }
        Ok(fkeys)
    }

    async fn get_table_ddl(&self, pool: &sqlx::Pool<Any>, database: &str, table: &str) -> Result<String> {
        let mut is_view = false;
        if let Ok(row) = sqlx::query("SELECT c.relkind = 'v' FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2")
            .bind(database)
            .bind(table)
            .fetch_one(pool).await {
            is_view = row.try_get(0).unwrap_or(false);
        }
        if is_view {
            let query_result = sqlx::query("SELECT pg_get_viewdef(c.oid) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2")
                .bind(database)
                .bind(table)
                .fetch_one(pool).await;
            if let Ok(row) = query_result {
                let view_def: String = row.try_get(0)?;
                return Ok(format!("CREATE VIEW \"{}\".\"{}\" AS\n{}", database, table, view_def));
            }
        }

        // 拼凑 DDL (PG 降级)
        let cols = self.describe_table(pool, database, table).await?;
        let mut ddl = format!("CREATE TABLE \"{}\".\"{}\" (\n", database, table);
        let mut pk_cols = Vec::new();
        for (i, col) in cols.iter().enumerate() {
            let nullable_str = if col.nullable { " NULL" } else { " NOT NULL" };
            let def_str = if let Some(ref d) = col.default_value {
                format!(" DEFAULT {}", d)
            } else {
                "".to_string()
            };
            let comma = if i == cols.len() - 1 && pk_cols.is_empty() {
                ""
            } else {
                ","
            };
            ddl.push_str(&format!(
                "  \"{}\" {}{}{}{}\n",
                col.name, col.type_name, nullable_str, def_str, comma
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

    async fn get_schema_fingerprint(&self, pool: &sqlx::Pool<Any>, database: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT COUNT(*), SUM(LENGTH(table_name)) FROM information_schema.tables WHERE table_schema = $1")
            .bind(database)
            .fetch_one(pool)
            .await?;
        let count: i64 = row.try_get(0)?;
        let len_sum: Option<i64> = row.try_get(1).ok().flatten();
        Ok(Some(format!("{}-{}", count, len_sum.unwrap_or(0))))
    }
}
