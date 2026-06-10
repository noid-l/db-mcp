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

pub mod dialect;
pub mod mysql;
pub mod postgres;
pub mod sqlite;
pub mod sqlserver;

pub use dialect::{DbDialect, get_dialect};

pub struct BaseConnector {
    db_type: String,
    config: crate::config::DataSourceConfig,
    pool: sqlx::Pool<Any>,
    query_timeout: u64,
    max_result_set_size_bytes: i64,
    sql_validator: crate::security::SqlValidator,
    dialect: Box<dyn DbDialect>,
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
        let dialect = get_dialect(db_type)?;

        // 建立动态连接池
        let pool = sqlx::Pool::<Any>::connect(&dsn).await?;

        Ok(Self {
            db_type: db_type.to_string(),
            config,
            pool,
            query_timeout,
            max_result_set_size_bytes,
            sql_validator,
            dialect,
        })
    }

    pub fn db_type(&self) -> &str {
        &self.db_type
    }

    pub async fn test_connection(&self) -> bool {
        matches!(
            tokio::time::timeout(Duration::from_secs(2), self.pool.acquire()).await,
            Ok(Ok(_conn))
        )
    }

    pub fn get_version(&self) -> String {
        format!("{} database", self.db_type)
    }

    async fn get_default_database_name(&self) -> String {
        if let Some(query) = self.dialect.get_default_db_query() {
            let res = sqlx::query(query)
                .fetch_one(&self.pool)
                .await
                .ok()
                .and_then(|row| row.try_get::<String, _>(0).ok());
            if let Some(name) = res {
                return name;
            }
        }

        self.config
            .database
            .clone()
            .unwrap_or_else(|| "main".to_string())
    }

    pub async fn list_databases(&self) -> Result<Vec<String>> {
        self.dialect.list_databases(&self.pool).await
    }

    pub async fn get_schema_fingerprint(&self, database: Option<String>) -> Result<Option<String>> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };
        self.dialect.get_schema_fingerprint(&self.pool, &db).await
    }

    pub async fn list_tables(&self, database: Option<String>) -> Result<Vec<TableMetadata>> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };
        self.dialect.list_tables(&self.pool, &db).await
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
        self.dialect.describe_table(&self.pool, &db, table).await
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
        self.dialect.list_indexes(&self.pool, &db, table).await
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
        self.dialect.get_imported_keys(&self.pool, &db, table).await
    }

    pub async fn get_table_ddl(&self, database: Option<String>, table: &str) -> Result<String> {
        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };
        self.dialect.get_table_ddl(&self.pool, &db, table).await
    }

    pub async fn execute_query(&self, sql_str: &str, max_rows: usize) -> Result<QueryResult> {
        // 只读性拦截
        self.sql_validator.validate(sql_str)?;

        let mut sql_to_execute = sql_str.trim().to_string();
        let sql_lower = sql_to_execute.to_lowercase();

        // 自动追加 LIMIT (通过 dialect 决定)
        if sql_lower.starts_with("select") && !sql_lower.contains("limit") {
            sql_to_execute = self.dialect.apply_limit(&sql_to_execute, max_rows);
        }

        let start_time = std::time::Instant::now();

        // 我们需要使用 sqlx::query 执行，并提取 metadata 及 row
        let rows = tokio::time::timeout(
            Duration::from_secs(self.query_timeout),
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
        if where_clause
            .as_ref()
            .is_some_and(|w| crate::config::contains_subquery(w))
        {
            return Err(anyhow::anyhow!(
                "Subqueries are not allowed in count_rows filter"
            ));
        }

        let db = match database {
            Some(d) => d,
            None => self.get_default_database_name().await,
        };

        let table_name = self.dialect.format_table_name(&db, table);

        let mut sql = format!("SELECT COUNT(*) FROM {}", table_name);
        if let Some(w) = where_clause.as_ref().filter(|w| !w.trim().is_empty()) {
            sql = format!("{} WHERE {}", sql, w);
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

        let table_name = self.dialect.format_table_name(&db, table);

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

impl Default for DataSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
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
