use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::server::McpServer;
use crate::types::{InputSchema, Property, Tool};

pub fn tool_definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "refresh_schema".to_string(),
            description: "强制刷新指定数据源的本地 Schema 缓存（包括表结构和字段信息），使本地缓存与目标数据库保持同步。".to_string(),
            input_schema: InputSchema {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = HashMap::new();
                    map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                    map
                },
                required: vec!["dataSource".to_string()],
            },
        },
        Tool {
            name: "search_schema".to_string(),
            description: "在指定数据源的本地 Schema 缓存中，通过关键词模糊搜索匹配的表名、列名以及注释，快速定位包含敏感信息或关联业务的表和列。".to_string(),
            input_schema: InputSchema {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = HashMap::new();
                    map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                    map.insert("query".to_string(), Property { prop_type: "string".to_string(), description: "要搜索的关键字".to_string() });
                    map
                },
                required: vec!["dataSource".to_string(), "query".to_string()],
            },
        },
    ]
}

impl McpServer {
    pub(crate) async fn handle_refresh_schema(&self, ds: &str, args: &Value) -> Result<String> {
        let conn = self.registry.get_connector(ds)?;
        let db = args
            .get("database")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(ref store) = self.metadata_store {
            let fetched_tables = conn.list_tables(db.clone()).await?;
            let mut columns = Vec::new();
            for t in &fetched_tables {
                if let Ok(cols) = conn.describe_table(db.clone(), &t.name).await {
                    columns.push((t.name.clone(), cols));
                }
            }
            let current_fp = conn.get_schema_fingerprint(db).await.ok().flatten();
            store
                .update_schema_cache(ds, &fetched_tables, &columns, current_fp.as_deref())
                .await?;
            Ok(format!(
                "Successfully refreshed schema cache for datasource '{}'",
                ds
            ))
        } else {
            Err(anyhow::anyhow!("Metadata cache store is not initialized"))
        }
    }

    pub(crate) async fn handle_search_schema(&self, ds: &str, args: &Value) -> Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing parameter 'query'"))?;
        if let Some(ref store) = self.metadata_store {
            let results = store.search_cached_schema(ds, query).await?;
            let mut formatted = Vec::new();
            for (tbl, col, comment) in results {
                formatted.push(json!({
                    "table": tbl,
                    "column": col,
                    "match": comment
                }));
            }
            Ok(serde_json::to_string_pretty(&formatted)?)
        } else {
            Err(anyhow::anyhow!("Metadata cache store is not initialized"))
        }
    }
}
