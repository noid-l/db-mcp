use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

use crate::server::McpServer;
use crate::types::{InputSchema, Property, Tool};

pub fn tool_definitions(server: &McpServer) -> Vec<Tool> {
    vec![
        Tool {
            name: "list_tables".to_string(),
            description: "列出指定数据源和数据库下的表和视图列表，支持通过通配符过滤。".to_string(),
            input_schema: InputSchema {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = HashMap::new();
                    map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                    map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库/Schema 名称，不指定则使用默认值".to_string() });
                    map.insert("pattern".to_string(), Property { prop_type: "string".to_string(), description: "表名过滤正则或通配符，支持 % 和 _".to_string() });
                    map
                },
                required: vec!["dataSource".to_string()],
            },
        },
        Tool {
            name: "describe_table".to_string(),
            description: "获取指定表的字段结构，包括列名、类型、是否为空、默认值、是否为主键和列注释。".to_string(),
            input_schema: server.table_schema(),
        },
        Tool {
            name: "list_indexes".to_string(),
            description: "列出指定表的索引信息，包括索引名称、包含的列名、是否唯一等属性。".to_string(),
            input_schema: server.table_schema(),
        },
        Tool {
            name: "get_foreign_keys".to_string(),
            description: "获取指定表的外键关联引用关系，展示主键表、主键列、外键列等关联细节。".to_string(),
            input_schema: server.table_schema(),
        },
        Tool {
            name: "get_table_ddl".to_string(),
            description: "获取指定表或视图的 DDL 建表/建视图语句（方言级别自动适配，不支持的数据库会平滑降级）。".to_string(),
            input_schema: server.table_schema(),
        },
    ]
}

impl McpServer {
    pub(crate) async fn handle_list_tables(&self, ds: &str, args: &Value) -> Result<String> {
        let db = args
            .get("database")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let pattern = args.get("pattern").and_then(|v| v.as_str());
        let conn = self.registry.get_connector(ds)?;

        let mut tables = None;
        let ttl_secs = 14400; // 4 hours

        if let Some(ref store) = self.metadata_store
            && let Ok(Some((last_updated, cached_fp))) = store.get_cache_status(ds).await
        {
            let now = chrono::Utc::now().timestamp();
            if now - last_updated < ttl_secs
                && let Ok(current_fp) = conn.get_schema_fingerprint(db.clone()).await
                && cached_fp == current_fp
                && let Ok(Some(cached_tables)) = store.get_cached_tables(ds).await
            {
                tables = Some(cached_tables);
            }
        }

        let mut tables = match tables {
            Some(t) => t,
            None => {
                let fetched_tables = conn.list_tables(db.clone()).await?;
                if let Some(ref store) = self.metadata_store {
                    let store_clone = store.clone();
                    let ds_clone = ds.to_string();
                    let db_clone = db.clone();
                    let conn_clone = conn.clone();
                    let fetched_tables_clone = fetched_tables.clone();

                    tokio::spawn(async move {
                        let mut columns = Vec::new();
                        for t in &fetched_tables_clone {
                            if let Ok(cols) =
                                conn_clone.describe_table(db_clone.clone(), &t.name).await
                            {
                                columns.push((t.name.clone(), cols));
                            }
                        }
                        let current_fp = conn_clone
                            .get_schema_fingerprint(db_clone)
                            .await
                            .ok()
                            .flatten();
                        let _ = store_clone
                            .update_schema_cache(
                                &ds_clone,
                                &fetched_tables_clone,
                                &columns,
                                current_fp.as_deref(),
                            )
                            .await;
                    });
                }
                fetched_tables
            }
        };

        if let Some(pat) = pattern {
            let regex_str = db_core::config::wildcard_to_regex(pat);
            if let Ok(reg) = regex::Regex::new(&format!("(?i){}", regex_str)) {
                tables.retain(|t| reg.is_match(&t.name));
            }
        }
        Ok(serde_json::to_string_pretty(&tables)?)
    }

    pub(crate) async fn handle_describe_table(&self, ds: &str, args: &Value) -> Result<String> {
        let (table, db) = self.get_table_and_db(args)?;
        let conn = self.registry.get_connector(ds)?;

        let mut cols = None;
        let ttl_secs = 14400;

        if let Some(ref store) = self.metadata_store
            && let Ok(Some((last_updated, cached_fp))) = store.get_cache_status(ds).await
        {
            let now = chrono::Utc::now().timestamp();
            if now - last_updated < ttl_secs
                && let Ok(current_fp) = conn.get_schema_fingerprint(db.clone()).await
                && cached_fp == current_fp
                && let Ok(Some(cached_cols)) = store.get_cached_columns(ds, table).await
            {
                cols = Some(cached_cols);
            }
        }

        let cols = match cols {
            Some(c) => c,
            None => {
                let fetched_cols = conn.describe_table(db.clone(), table).await?;
                if let Some(ref store) = self.metadata_store {
                    let store_clone = store.clone();
                    let ds_clone = ds.to_string();
                    let db_clone = db.clone();
                    let conn_clone = conn.clone();
                    tokio::spawn(async move {
                        if let Ok(fetched_tables) = conn_clone.list_tables(db_clone.clone()).await {
                            let mut columns = Vec::new();
                            for t in &fetched_tables {
                                if let Ok(cols) =
                                    conn_clone.describe_table(db_clone.clone(), &t.name).await
                                {
                                    columns.push((t.name.clone(), cols));
                                }
                            }
                            let current_fp = conn_clone
                                .get_schema_fingerprint(db_clone)
                                .await
                                .ok()
                                .flatten();
                            let _ = store_clone
                                .update_schema_cache(
                                    &ds_clone,
                                    &fetched_tables,
                                    &columns,
                                    current_fp.as_deref(),
                                )
                                .await;
                        }
                    });
                }
                fetched_cols
            }
        };

        Ok(serde_json::to_string_pretty(&cols)?)
    }

    pub(crate) async fn handle_list_indexes(&self, ds: &str, args: &Value) -> Result<String> {
        let (table, db) = self.get_table_and_db(args)?;
        let conn = self.registry.get_connector(ds)?;
        let idxs = conn.list_indexes(db, table).await?;
        Ok(serde_json::to_string_pretty(&idxs)?)
    }

    pub(crate) async fn handle_get_foreign_keys(&self, ds: &str, args: &Value) -> Result<String> {
        let (table, db) = self.get_table_and_db(args)?;
        let conn = self.registry.get_connector(ds)?;
        let fkeys = conn.get_imported_keys(db, table).await?;
        Ok(serde_json::to_string_pretty(&fkeys)?)
    }

    pub(crate) async fn handle_get_table_ddl(&self, ds: &str, args: &Value) -> Result<String> {
        let (table, db) = self.get_table_and_db(args)?;
        let conn = self.registry.get_connector(ds)?;
        let ddl = conn.get_table_ddl(db, table).await?;
        Ok(ddl)
    }
}
