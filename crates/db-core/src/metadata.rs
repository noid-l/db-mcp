use sqlx::{SqlitePool, Row};
use crate::config::DataSourceConfig;
use crate::db::{TableMetadata, ColumnMetadata};
use anyhow::Result;

pub struct MetadataStore {
    pool: SqlitePool,
}

impl MetadataStore {
    /// 初始化连接并自动建表
    pub async fn new(db_path: &str) -> Result<Self> {
        if db_path != ":memory:" {
            if let Some(parent) = std::path::Path::new(db_path).parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        
        let connection_str = format!("sqlite://{}", db_path);
        let pool = SqlitePool::connect(&connection_str).await?;
        
        // 自动初始化元数据表
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS data_sources (
                name TEXT PRIMARY KEY,
                db_type TEXT NOT NULL,
                config_json TEXT NOT NULL
            )"
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_cache_meta (
                datasource_name TEXT PRIMARY KEY,
                last_updated_at INTEGER NOT NULL,
                schema_fingerprint TEXT
            )"
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tables_cache (
                datasource_name TEXT NOT NULL,
                table_name TEXT NOT NULL,
                table_type TEXT NOT NULL,
                comment TEXT NOT NULL,
                PRIMARY KEY (datasource_name, table_name)
            )"
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS columns_cache (
                datasource_name TEXT NOT NULL,
                table_name TEXT NOT NULL,
                column_name TEXT NOT NULL,
                type_name TEXT NOT NULL,
                nullable INTEGER NOT NULL,
                default_value TEXT,
                primary_key INTEGER NOT NULL,
                comment TEXT NOT NULL,
                PRIMARY KEY (datasource_name, table_name, column_name)
            )"
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    /// 缓存数据源配置到 SQLite (如果已存在则执行更新)
    pub async fn save_data_source(&self, name: &str, db_type: &str, config: &DataSourceConfig) -> Result<()> {
        let config_json = serde_json::to_string(config)?;
        sqlx::query(
            "INSERT INTO data_sources (name, db_type, config_json)
             VALUES (?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET
                db_type = excluded.db_type,
                config_json = excluded.config_json"
        )
        .bind(name)
        .bind(db_type)
        .bind(config_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 从缓存中删除数据源
    pub async fn remove_data_source(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM data_sources WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 列出所有被缓存的数据源配置
    pub async fn list_data_sources(&self) -> Result<Vec<(String, String, DataSourceConfig)>> {
        let rows = sqlx::query("SELECT name, db_type, config_json FROM data_sources")
            .fetch_all(&self.pool)
            .await?;
        
        let mut list = Vec::new();
        for row in rows {
            let name: String = row.try_get("name")?;
            let db_type: String = row.try_get("db_type")?;
            let config_json: String = row.try_get("config_json")?;
            let config: DataSourceConfig = serde_json::from_str(&config_json)?;
            list.push((name, db_type, config));
        }
        Ok(list)
    }

    /// 获取数据源的 Schema 缓存状态 (最后更新时间及指纹)
    pub async fn get_cache_status(&self, datasource_name: &str) -> Result<Option<(i64, Option<String>)>> {
        let row = sqlx::query(
            "SELECT last_updated_at, schema_fingerprint FROM schema_cache_meta WHERE datasource_name = ?"
        )
        .bind(datasource_name)
        .fetch_optional(&self.pool)
        .await?;
        
        if let Some(r) = row {
            let last_updated: i64 = r.try_get(0)?;
            let fingerprint: Option<String> = r.try_get(1)?;
            Ok(Some((last_updated, fingerprint)))
        } else {
            Ok(None)
        }
    }

    /// 写入/更新某数据源的全部 Schema 缓存数据 (事务操作)
    pub async fn update_schema_cache(
        &self,
        datasource_name: &str,
        tables: &[TableMetadata],
        columns: &[(String, Vec<ColumnMetadata>)],
        fingerprint: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        
        // 删去旧表
        sqlx::query("DELETE FROM tables_cache WHERE datasource_name = ?")
            .bind(datasource_name)
            .execute(&mut *tx)
            .await?;
            
        // 删去旧列
        sqlx::query("DELETE FROM columns_cache WHERE datasource_name = ?")
            .bind(datasource_name)
            .execute(&mut *tx)
            .await?;
            
        // 插入新表
        for t in tables {
            sqlx::query(
                "INSERT INTO tables_cache (datasource_name, table_name, table_type, comment)
                 VALUES (?, ?, ?, ?)"
            )
            .bind(datasource_name)
            .bind(&t.name)
            .bind(&t.table_type)
            .bind(&t.comment)
            .execute(&mut *tx)
            .await?;
        }
        
        // 插入新列
        for (table_name, cols) in columns {
            for col in cols {
                sqlx::query(
                    "INSERT INTO columns_cache (datasource_name, table_name, column_name, type_name, nullable, default_value, primary_key, comment)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
                )
                .bind(datasource_name)
                .bind(table_name)
                .bind(&col.name)
                .bind(&col.type_name)
                .bind(if col.nullable { 1 } else { 0 })
                .bind(&col.default_value)
                .bind(if col.primary_key { 1 } else { 0 })
                .bind(&col.comment)
                .execute(&mut *tx)
                .await?;
            }
        }
        
        // 更新 meta
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO schema_cache_meta (datasource_name, last_updated_at, schema_fingerprint)
             VALUES (?, ?, ?)
             ON CONFLICT(datasource_name) DO UPDATE SET
                last_updated_at = excluded.last_updated_at,
                schema_fingerprint = excluded.schema_fingerprint"
        )
        .bind(datasource_name)
        .bind(now)
        .bind(fingerprint)
        .execute(&mut *tx)
        .await?;
        
        tx.commit().await?;
        Ok(())
    }

    /// 获取缓存的表信息
    pub async fn get_cached_tables(&self, datasource_name: &str) -> Result<Option<Vec<TableMetadata>>> {
        let meta_exists = sqlx::query("SELECT 1 FROM schema_cache_meta WHERE datasource_name = ?")
            .bind(datasource_name)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
            
        if !meta_exists {
            return Ok(None);
        }
        
        let rows = sqlx::query(
            "SELECT table_name, table_type, comment FROM tables_cache WHERE datasource_name = ?"
        )
        .bind(datasource_name)
        .fetch_all(&self.pool)
        .await?;
            
        let mut list = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let t_type: String = row.try_get(1)?;
            let comment: String = row.try_get(2)?;
            list.push(TableMetadata {
                name,
                catalog: None,
                schema: Some("main".to_string()),
                table_type: t_type,
                comment,
            });
        }
        Ok(Some(list))
    }

    /// 获取指定表下的缓存字段信息
    pub async fn get_cached_columns(&self, datasource_name: &str, table_name: &str) -> Result<Option<Vec<ColumnMetadata>>> {
        let meta_exists = sqlx::query("SELECT 1 FROM schema_cache_meta WHERE datasource_name = ?")
            .bind(datasource_name)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
            
        if !meta_exists {
            return Ok(None);
        }
        
        let rows = sqlx::query(
            "SELECT column_name, type_name, nullable, default_value, primary_key, comment 
             FROM columns_cache 
             WHERE datasource_name = ? AND table_name = ?"
        )
        .bind(datasource_name)
        .bind(table_name)
        .fetch_all(&self.pool)
        .await?;
        
        let mut list = Vec::new();
        for row in rows {
            let column_name: String = row.try_get(0)?;
            let type_name: String = row.try_get(1)?;
            let nullable: i64 = row.try_get(2)?;
            let default_value: Option<String> = row.try_get(3)?;
            let primary_key: i64 = row.try_get(4)?;
            let comment: String = row.try_get(5)?;
            
            list.push(ColumnMetadata {
                name: column_name,
                sql_type: 0,
                type_name,
                size: 0,
                digits: None,
                nullable: nullable != 0,
                default_value,
                primary_key: primary_key != 0,
                comment,
            });
        }
        Ok(Some(list))
    }

    /// 全文模糊检索字段或表
    pub async fn search_cached_schema(&self, datasource_name: &str, query: &str) -> Result<Vec<(String, String, String)>> {
        let search_pattern = format!("%{}%", query);
        
        // 1. 搜表名或表注释
        let table_rows = sqlx::query(
            "SELECT table_name, comment FROM tables_cache 
             WHERE datasource_name = ? AND (table_name LIKE ? OR comment LIKE ?)"
        )
        .bind(datasource_name)
        .bind(&search_pattern)
        .bind(&search_pattern)
        .fetch_all(&self.pool)
        .await?;
        
        let mut results = Vec::new();
        for row in table_rows {
            let table_name: String = row.try_get(0)?;
            let comment: String = row.try_get(1)?;
            results.push((table_name, "".to_string(), format!("Table match: {}", comment)));
        }
        
        // 2. 搜列名或列注释
        let col_rows = sqlx::query(
            "SELECT table_name, column_name, comment FROM columns_cache 
             WHERE datasource_name = ? AND (column_name LIKE ? OR comment LIKE ?)"
        )
        .bind(datasource_name)
        .bind(&search_pattern)
        .bind(&search_pattern)
        .fetch_all(&self.pool)
        .await?;
        
        for row in col_rows {
            let table_name: String = row.try_get(0)?;
            let column_name: String = row.try_get(1)?;
            let comment: String = row.try_get(2)?;
            results.push((table_name, column_name, format!("Column match: {}", comment)));
        }
        
        Ok(results)
    }

    /// 标记/强行失效缓存
    pub async fn invalidate_schema_cache(&self, datasource_name: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM schema_cache_meta WHERE datasource_name = ?")
            .bind(datasource_name)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM tables_cache WHERE datasource_name = ?")
            .bind(datasource_name)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM columns_cache WHERE datasource_name = ?")
            .bind(datasource_name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metadata_store() {
        let store = MetadataStore::new(":memory:").await.unwrap();
        
        let config = DataSourceConfig {
            db_type: "SQLITE".to_string(),
            jdbc_url: None,
            host: None,
            port: None,
            database: Some("test.db".to_string()),
            username: None,
            password: None,
            dsn: None,
            properties: None,
        };

        // 1. 保存
        store.save_data_source("test_ds", "SQLITE", &config).await.unwrap();

        // 2. 列表查询
        let list = store.list_data_sources().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "test_ds");
        assert_eq!(list[0].1, "SQLITE");
        assert_eq!(list[0].2.database.as_deref(), Some("test.db"));

        // 3. 删除
        store.remove_data_source("test_ds").await.unwrap();
        let list = store.list_data_sources().await.unwrap();
        assert_eq!(list.len(), 0);
    }

    #[tokio::test]
    async fn test_schema_cache() {
        let store = MetadataStore::new(":memory:").await.unwrap();
        
        let tables = vec![TableMetadata {
            name: "users".to_string(),
            catalog: None,
            schema: None,
            table_type: "TABLE".to_string(),
            comment: "用户表".to_string(),
        }];
        
        let cols = vec![
            ColumnMetadata {
                name: "id".to_string(),
                sql_type: 0,
                type_name: "INTEGER".to_string(),
                size: 0,
                digits: None,
                nullable: false,
                default_value: None,
                primary_key: true,
                comment: "主键".to_string(),
            },
            ColumnMetadata {
                name: "name".to_string(),
                sql_type: 0,
                type_name: "TEXT".to_string(),
                size: 0,
                digits: None,
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: "姓名".to_string(),
            }
        ];
        
        let columns = vec![("users".to_string(), cols)];
        
        // 更新缓存
        store.update_schema_cache("ds1", &tables, &columns, Some("v1")).await.unwrap();
        
        // 状态检查
        let status = store.get_cache_status("ds1").await.unwrap().unwrap();
        assert_eq!(status.1.as_deref(), Some("v1"));
        
        // 获取缓存表
        let cached_tables = store.get_cached_tables("ds1").await.unwrap().unwrap();
        assert_eq!(cached_tables.len(), 1);
        assert_eq!(cached_tables[0].name, "users");
        
        // 获取缓存字段
        let cached_cols = store.get_cached_columns("ds1", "users").await.unwrap().unwrap();
        assert_eq!(cached_cols.len(), 2);
        assert_eq!(cached_cols[1].name, "name");
        assert_eq!(cached_cols[1].comment, "姓名");
        
        // 搜索测试
        let search_res = store.search_cached_schema("ds1", "姓名").await.unwrap();
        assert_eq!(search_res.len(), 1);
        assert_eq!(search_res[0].0, "users");
        assert_eq!(search_res[0].1, "name");
        
        // 失效缓存
        store.invalidate_schema_cache("ds1").await.unwrap();
        let status = store.get_cache_status("ds1").await.unwrap();
        assert!(status.is_none());
    }
}
