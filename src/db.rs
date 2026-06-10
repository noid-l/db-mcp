#![allow(clippy::collapsible_if)]
#![allow(clippy::match_like_matches_macro)]

use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Column, Row, TypeInfo, any::Any};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Serialize, Clone)]
pub struct TableMetadata {
    pub name: String,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    #[serde(rename = "type")]
    pub table_type: String,
    pub comment: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ColumnMetadata {
    pub name: String,
    #[serde(rename = "sqlType")]
    pub sql_type: i32,
    #[serde(rename = "type")]
    pub type_name: String,
    pub size: i32,
    pub digits: Option<i32>,
    pub nullable: bool,
    #[serde(rename = "defaultValue")]
    pub default_value: Option<String>,
    #[serde(rename = "primaryKey")]
    pub primary_key: bool,
    pub comment: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ColumnMeta {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(rename = "displaySize")]
    pub display_size: i32,
    pub nullable: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<HashMap<String, Value>>,
    #[serde(rename = "rowCount")]
    pub row_count: usize,
    #[serde(rename = "executionTimeMs")]
    pub execution_time_ms: i64,
    pub truncated: bool,
}

pub struct BaseConnector {
    db_type: String,
    config: crate::config::DataSourceConfig,
    pool: sqlx::Pool<Any>,
    query_timeout: u64,
    max_result_set_size_bytes: i64,
    sql_validator: crate::security::SqlValidator,
}

impl BaseConnector {
    pub async fn new(
        db_type: &str,
        config: crate::config::DataSourceConfig,
        query_timeout: u64,
        max_result_set_size_bytes: i64,
        sql_validator: crate::security::SqlValidator,
    ) -> Result<Self> {
        let dsn = crate::config::convert_to_dsn(&config)?;

        // 建立动态连接池
        let pool = sqlx::Pool::<Any>::connect(&dsn).await?;

        Ok(Self {
            db_type: db_type.to_string(),
            config,
            pool,
            query_timeout,
            max_result_set_size_bytes,
            sql_validator,
        })
    }

    pub fn db_type(&self) -> &str {
        &self.db_type
    }

    pub async fn test_connection(&self) -> bool {
        match tokio::time::timeout(Duration::from_secs(2), self.pool.acquire()).await {
            Ok(Ok(_conn)) => true,
            _ => false,
        }
    }

    pub fn get_version(&self) -> String {
        // 由于 sqlx 的 version 难以跨驱动简单读取，我们在运行时根据类型做简单查询
        // 或者直接返回一个默认提示
        format!("{} database", self.db_type)
    }

    async fn get_default_database_name(&self) -> String {
        let db_type_upper = self.db_type.to_uppercase();
        let query = match db_type_upper.as_str() {
            "MYSQL" | "MARIADB" | "GBASE" => "SELECT DATABASE()",
            "POSTGRESQL" | "KINGBASE" | "HIGHGO" => "SELECT current_database()",
            "SQLSERVER" => "SELECT DB_NAME()",
            _ => "",
        };

        if !query.is_empty() {
            if let Ok(row) = sqlx::query(query).fetch_one(&self.pool).await {
                if let Ok(name) = row.try_get::<String, _>(0) {
                    return name;
                }
            }
        }

        self.config
            .database
            .clone()
            .unwrap_or_else(|| "main".to_string())
    }

    pub async fn list_databases(&self) -> Result<Vec<String>> {
        let db_type_upper = self.db_type.to_uppercase();
        let query = match db_type_upper.as_str() {
            "MYSQL" | "MARIADB" | "GBASE" => "SHOW DATABASES",
            "POSTGRESQL" | "KINGBASE" | "HIGHGO" => {
                "SELECT datname FROM pg_database WHERE datistemplate = false"
            }
            "SQLSERVER" => {
                "SELECT name FROM sys.databases WHERE name NOT IN ('master', 'tempdb', 'model', 'msdb')"
            }
            "SQLITE" => return Ok(vec!["main".to_string()]),
            _ => return Err(anyhow::anyhow!("Unsupported database type")),
        };

        let rows = sqlx::query(query).fetch_all(&self.pool).await?;
        let mut dbs = Vec::new();
        for row in rows {
            if let Ok(name) = row.try_get::<String, _>(0) {
                dbs.push(name);
            }
        }
        Ok(dbs)
    }

    pub async fn list_tables(&self, database: Option<String>) -> Result<Vec<TableMetadata>> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let db_type_upper = self.db_type.to_uppercase();
        let mut tables = Vec::new();

        match db_type_upper.as_str() {
            "MYSQL" | "MARIADB" | "GBASE" => {
                let rows = sqlx::query("SELECT TABLE_NAME, TABLE_TYPE, COALESCE(TABLE_COMMENT, '') FROM information_schema.TABLES WHERE TABLE_SCHEMA = ?")
                    .bind(&db)
                    .fetch_all(&self.pool).await?;
                for row in rows {
                    let name: String = row.try_get(0)?;
                    let t_type: String = row.try_get(1)?;
                    let comment: String = row.try_get(2)?;
                    tables.push(TableMetadata {
                        name,
                        catalog: None,
                        schema: Some(db.clone()),
                        table_type: t_type,
                        comment,
                    });
                }
            }
            "POSTGRESQL" | "KINGBASE" | "HIGHGO" => {
                let rows = sqlx::query("SELECT c.relname AS table_name, CASE WHEN c.relkind = 'r' THEN 'TABLE' ELSE 'VIEW' END AS table_type, COALESCE(obj_description(c.oid, 'pg_class'), '') AS comment FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relkind IN ('r', 'v')")
                    .bind(&db)
                    .fetch_all(&self.pool).await?;
                for row in rows {
                    let name: String = row.try_get(0)?;
                    let t_type: String = row.try_get(1)?;
                    let comment: String = row.try_get(2)?;
                    tables.push(TableMetadata {
                        name,
                        catalog: None,
                        schema: Some(db.clone()),
                        table_type: t_type,
                        comment,
                    });
                }
            }
            "SQLITE" => {
                let rows = sqlx::query("SELECT name, 'TABLE' as type FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' UNION ALL SELECT name, 'VIEW' as type FROM sqlite_master WHERE type='view'")
                    .fetch_all(&self.pool).await?;
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
            }
            "SQLSERVER" => {
                let rows = sqlx::query("SELECT table_name, table_type FROM information_schema.tables WHERE table_schema = @p1")
                    .bind(&db)
                    .fetch_all(&self.pool).await?;
                for row in rows {
                    let name: String = row.try_get(0)?;
                    let t_type: String = row.try_get(1)?;
                    tables.push(TableMetadata {
                        name,
                        catalog: None,
                        schema: Some(db.clone()),
                        table_type: t_type,
                        comment: "".to_string(),
                    });
                }
            }
            _ => return Err(anyhow::anyhow!("Unsupported database type")),
        }

        Ok(tables)
    }

    pub async fn describe_table(
        &self,
        database: Option<String>,
        table: &str,
    ) -> Result<Vec<ColumnMetadata>> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let db_type_upper = self.db_type.to_uppercase();
        let mut columns = Vec::new();

        match db_type_upper.as_str() {
            "MYSQL" | "MARIADB" | "GBASE" => {
                let rows = sqlx::query("SELECT COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_COMMENT, CASE WHEN COLUMN_KEY = 'PRI' THEN 1 ELSE 0 END AS IS_PK FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION")
                    .bind(&db)
                    .bind(table)
                    .fetch_all(&self.pool).await?;
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
            }
            "POSTGRESQL" | "KINGBASE" | "HIGHGO" => {
                let rows = sqlx::query("SELECT a.attname AS column_name, format_type(a.atttypid, a.atttypmod) AS type_name, a.attnotnull = false AS nullable, COALESCE(pg_get_expr(ad.adbin, ad.adrelid), '') AS default_value, COALESCE(i.indisprimary, false) AS is_pk, COALESCE(col_description(a.attrelid, a.attnum), '') AS comment FROM pg_attribute a LEFT JOIN pg_attrdef ad ON a.attrelid = ad.adrelid AND a.attnum = ad.adnum LEFT JOIN pg_index i ON a.attrelid = i.indrelid AND a.attnum = any(i.indkey) JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum")
                    .bind(&db)
                    .bind(table)
                    .fetch_all(&self.pool).await?;
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
            }
            "SQLITE" => {
                let rows = sqlx::query(&format!("PRAGMA table_info({})", table))
                    .fetch_all(&self.pool)
                    .await?;
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
            }
            "SQLSERVER" => {
                let rows = sqlx::query("SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, CASE WHEN k.column_name IS NOT NULL THEN 1 ELSE 0 END AS is_pk FROM information_schema.columns c LEFT JOIN (SELECT ku.table_catalog, ku.table_schema, ku.table_name, ku.column_name FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage ku ON tc.constraint_name = ku.constraint_name WHERE tc.constraint_type = 'PRIMARY KEY') k ON c.table_catalog = k.table_catalog AND c.table_schema = k.table_schema AND c.table_name = k.table_name AND c.column_name = k.column_name WHERE c.table_schema = @p1 AND c.table_name = @p2 ORDER BY c.ordinal_position")
                    .bind(&db)
                    .bind(table)
                    .fetch_all(&self.pool).await?;
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
            }
            _ => return Err(anyhow::anyhow!("Unsupported database type")),
        }

        Ok(columns)
    }

    pub async fn list_indexes(
        &self,
        database: Option<String>,
        table: &str,
    ) -> Result<Vec<HashMap<String, Value>>> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let db_type_upper = self.db_type.to_uppercase();
        let mut indexes = Vec::new();

        match db_type_upper.as_str() {
            "MYSQL" | "MARIADB" | "GBASE" => {
                let rows = sqlx::query("SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, INDEX_TYPE, SEQ_IN_INDEX FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?")
                    .bind(&db)
                    .bind(table)
                    .fetch_all(&self.pool).await?;
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
            }
            "POSTGRESQL" | "KINGBASE" | "HIGHGO" => {
                let rows = sqlx::query("SELECT c2.relname AS index_name, i.indisunique = false AS non_unique, pg_get_indexdef(i.indexrelid) AS filter_condition FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid JOIN pg_class c2 ON c2.oid = i.indexrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2")
                    .bind(&db)
                    .bind(table)
                    .fetch_all(&self.pool).await?;
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
            }
            "SQLITE" => {
                let rows = sqlx::query(&format!("PRAGMA index_list({})", table))
                    .fetch_all(&self.pool)
                    .await?;
                let mut items = Vec::new();
                for row in rows {
                    let name: String = row.try_get(1)?;
                    let unique: i64 = row.try_get(2)?;
                    items.push((name, unique > 0));
                }

                for item in items {
                    let info_rows = sqlx::query(&format!("PRAGMA index_info({})", item.0))
                        .fetch_all(&self.pool)
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
            }
            _ => {}
        }

        Ok(indexes)
    }

    pub async fn get_imported_keys(
        &self,
        database: Option<String>,
        table: &str,
    ) -> Result<Vec<HashMap<String, Value>>> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let db_type_upper = self.db_type.to_uppercase();
        let mut fkeys = Vec::new();

        match db_type_upper.as_str() {
            "MYSQL" | "MARIADB" | "GBASE" => {
                let rows = sqlx::query("SELECT REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME, TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION, CONSTRAINT_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND REFERENCED_TABLE_NAME IS NOT NULL")
                    .bind(&db)
                    .bind(table)
                    .fetch_all(&self.pool).await?;
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
            }
            "POSTGRESQL" | "KINGBASE" | "HIGHGO" => {
                let rows = sqlx::query("SELECT conname AS constraint_name, att2.attname AS child_column, cl.relname AS parent_table, att.attname AS parent_column FROM pg_constraint c JOIN pg_class cl ON cl.oid = c.confrelid JOIN pg_attribute att2 ON att2.attrelid = c.conrelid AND att2.attnum = any(c.conkey) JOIN pg_attribute att ON att.attrelid = c.confrelid AND att.attnum = any(c.confkey) JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2 AND c.contype = 'f'")
                    .bind(&db)
                    .bind(table)
                    .fetch_all(&self.pool).await?;
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
            }
            "SQLITE" => {
                let rows = sqlx::query(&format!("PRAGMA foreign_key_list({})", table))
                    .fetch_all(&self.pool)
                    .await?;
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
            }
            _ => {}
        }

        Ok(fkeys)
    }

    pub async fn get_table_ddl(&self, database: Option<String>, table: &str) -> Result<String> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let db_type_upper = self.db_type.to_uppercase();

        match db_type_upper.as_str() {
            "SQLITE" => {
                let row = sqlx::query(
                    "SELECT sql FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?",
                )
                .bind(table)
                .fetch_one(&self.pool)
                .await?;
                let sql: String = row.try_get(0)?;
                Ok(sql)
            }
            "MYSQL" | "MARIADB" => {
                let q = format!("SHOW CREATE TABLE `{}`.`{}`", db, table);
                if let Ok(row) = sqlx::query(&q).fetch_one(&self.pool).await {
                    let sql: String = row.try_get(1)?;
                    return Ok(sql);
                }
                let q_view = format!("SHOW CREATE VIEW `{}`.`{}`", db, table);
                let row = sqlx::query(&q_view).fetch_one(&self.pool).await?;
                let sql: String = row.try_get(1)?;
                Ok(sql)
            }
            "POSTGRESQL" | "KINGBASE" | "HIGHGO" => {
                let mut is_view = false;
                if let Ok(row) = sqlx::query("SELECT c.relkind = 'v' FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2")
                    .bind(&db)
                    .bind(table)
                    .fetch_one(&self.pool).await {
                    is_view = row.try_get(0).unwrap_or(false);
                }
                if is_view {
                    if let Ok(row) = sqlx::query("SELECT pg_get_viewdef(c.oid) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2")
                        .bind(&db)
                        .bind(table)
                        .fetch_one(&self.pool).await {
                        let view_def: String = row.try_get(0)?;
                        return Ok(format!("CREATE VIEW \"{}\".\"{}\" AS\n{}", db, table, view_def));
                    }
                }

                // 拼凑 DDL (PG 降级)
                let cols = self.describe_table(Some(db.clone()), table).await?;
                let mut ddl = format!("CREATE TABLE \"{}\".\"{}\" (\n", db, table);
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
            _ => {
                // 默认拼接
                let cols = self.describe_table(Some(db.clone()), table).await?;
                let mut ddl = format!("CREATE TABLE \"{}\".\"{}\" (\n", db, table);
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
    }

    pub async fn execute_query(&self, sql_str: &str, max_rows: usize) -> Result<QueryResult> {
        // 只读性拦截
        self.sql_validator.validate(sql_str)?;

        let mut sql_to_execute = sql_str.trim().to_string();
        let sql_lower = sql_to_execute.to_lowercase();

        // 自动追加 LIMIT (针对支持的 MySQL, Postgres, SQLite)
        if sql_lower.starts_with("select") && !sql_lower.contains("limit") {
            let db_type_upper = self.db_type.to_uppercase();
            if db_type_upper == "MYSQL"
                || db_type_upper == "POSTGRESQL"
                || db_type_upper == "SQLITE"
                || db_type_upper == "MARIADB"
            {
                sql_to_execute = format!("{} LIMIT {}", sql_to_execute, max_rows);
            }
        }

        let start_time = std::time::Instant::now();

        // 我们需要使用 sqlx::query 执行，并提取 metadata 及 row
        let rows = tokio::time::timeout(
            std::time::Duration::from_secs(self.query_timeout),
            sqlx::query(&sql_to_execute).fetch_all(&self.pool),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Query execution timed out after {} seconds",
                self.query_timeout
            )
        })??;

        let duration = start_time.elapsed().as_millis() as i64;

        if rows.is_empty() {
            return Ok(QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                row_count: 0,
                execution_time_ms: duration,
                truncated: false,
            });
        }

        // 提取列元数据
        let first_row = &rows[0];
        let mut column_metas = Vec::new();
        for col in first_row.columns() {
            column_metas.push(ColumnMeta {
                name: col.name().to_string(),
                type_name: col.type_info().name().to_string(),
                display_size: 100,
                nullable: true,
            });
        }

        let mut row_maps = Vec::new();
        let mut current_size_bytes = 0;
        let mut truncated = false;
        let mut row_count = 0;

        for row in rows {
            if row_count >= max_rows {
                truncated = true;
                break;
            }

            let mut row_map = HashMap::new();
            let mut row_size = 0;

            for (i, col) in row.columns().iter().enumerate() {
                let val = get_any_value(&row, i);

                // 计算估算大小
                if let Value::String(ref s) = val {
                    row_size += s.len() as i64 * 2;
                } else {
                    row_size += 16;
                }

                row_map.insert(col.name().to_string(), val);
            }

            current_size_bytes += row_size;
            if current_size_bytes > self.max_result_set_size_bytes {
                truncated = true;
                break;
            }

            row_maps.push(row_map);
            row_count += 1;
        }

        Ok(QueryResult {
            columns: column_metas,
            rows: row_maps,
            row_count,
            execution_time_ms: duration,
            truncated,
        })
    }

    pub async fn count_rows(
        &self,
        database: Option<String>,
        table: &str,
        where_clause: Option<String>,
    ) -> Result<i64> {
        if let Some(ref w) = where_clause {
            if crate::config::contains_subquery(w) {
                return Err(anyhow::anyhow!(
                    "Subqueries are not allowed in count_rows filter"
                ));
            }
        }

        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let db_type_upper = self.db_type.to_uppercase();
        let table_name = if db_type_upper == "MYSQL" || db_type_upper == "MARIADB" {
            format!("`{}`.`{}`", db, table)
        } else {
            format!("\"{}\".\"{}\"", db, table)
        };

        let mut sql = format!("SELECT COUNT(*) FROM {}", table_name);
        if let Some(ref w) = where_clause {
            if !w.trim().is_empty() {
                sql = format!("{} WHERE {}", sql, w);
            }
        }

        // 只读校验
        self.sql_validator.validate(&sql)?;

        let row = sqlx::query(&sql).fetch_one(&self.pool).await?;
        let count: i64 = row.try_get(0)?;
        Ok(count)
    }

    pub async fn sample_data(
        &self,
        database: Option<String>,
        table: &str,
        limit: usize,
    ) -> Result<QueryResult> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let db_type_upper = self.db_type.to_uppercase();
        let table_name = if db_type_upper == "MYSQL" || db_type_upper == "MARIADB" {
            format!("`{}`.`{}`", db, table)
        } else {
            format!("\"{}\".\"{}\"", db, table)
        };

        let sql = format!("SELECT * FROM {}", table_name);
        self.execute_query(&sql, limit).await
    }
}

// 动态类型解析
fn get_any_value(row: &sqlx::any::AnyRow, index: usize) -> Value {
    let column = row.column(index);
    let type_name = column.type_info().name().to_uppercase();

    // 依次尝试获取，或者根据类型名识别
    match type_name.as_str() {
        "TEXT" | "VARCHAR" | "CHAR" | "VARCHAR2" | "NVARCHAR" | "NCHAR" | "STRING" | "BPCHAR" => {
            row.try_get::<String, _>(index)
                .map(|s| json!(s))
                .unwrap_or(Value::Null)
        }
        "INT" | "INTEGER" | "BIGINT" | "INT8" | "INT4" | "INT2" | "SMALLINT" | "TINYINT" => {
            if let Ok(val) = row.try_get::<i64, _>(index) {
                json!(val)
            } else if let Ok(val) = row.try_get::<i32, _>(index) {
                json!(val)
            } else if let Ok(val) = row.try_get::<i16, _>(index) {
                json!(val)
            } else {
                Value::Null
            }
        }
        "FLOAT" | "DOUBLE" | "REAL" | "DOUBLE PRECISION" | "NUMERIC" | "DECIMAL" => {
            if let Ok(val) = row.try_get::<f64, _>(index) {
                json!(val)
            } else if let Ok(val) = row.try_get::<f32, _>(index) {
                json!(val)
            } else {
                row.try_get::<String, _>(index)
                    .map(|s| json!(s))
                    .unwrap_or(Value::Null)
            }
        }
        "BOOLEAN" | "BOOL" => row
            .try_get::<bool, _>(index)
            .map(|b| json!(b))
            .unwrap_or(Value::Null),
        _ => {
            // 兜底尝试
            if let Ok(s) = row.try_get::<String, _>(index) {
                json!(s)
            } else if let Ok(n) = row.try_get::<i64, _>(index) {
                json!(n)
            } else if let Ok(f) = row.try_get::<f64, _>(index) {
                json!(f)
            } else if let Ok(b) = row.try_get::<bool, _>(index) {
                json!(b)
            } else {
                Value::Null
            }
        }
    }
}

// 数据源注册管理器
pub struct DataSourceRegistry {
    connectors: std::sync::RwLock<HashMap<String, std::sync::Arc<BaseConnector>>>,
}

impl DataSourceRegistry {
    pub fn new() -> Self {
        Self {
            connectors: std::sync::RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, name: String, connector: BaseConnector) {
        let mut map = self.connectors.write().unwrap();
        map.insert(name, std::sync::Arc::new(connector));
    }

    pub fn get_connector(&self, name: &str) -> Result<std::sync::Arc<BaseConnector>> {
        let map = self.connectors.read().unwrap();
        map.get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Datasource '{}' not found in registry", name))
    }

    pub fn unregister(&self, name: &str) -> bool {
        let mut map = self.connectors.write().unwrap();
        map.remove(name).is_some()
    }

    pub fn get_datasource_types(&self) -> HashMap<String, String> {
        let map = self.connectors.read().unwrap();
        map.iter()
            .map(|(k, v)| (k.clone(), v.db_type().to_uppercase()))
            .collect()
    }
}
