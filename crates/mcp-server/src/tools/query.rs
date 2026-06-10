use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

use crate::server::McpServer;
use crate::types::{InputSchema, Property, Tool};

pub fn tool_definitions(server: &McpServer) -> Vec<Tool> {
    vec![
        Tool {
            name: "execute_query".to_string(),
            description: "在指定数据源上执行只读 SQL 查询（只支持 SELECT/SHOW/EXPLAIN 等读操作）。为了防止大数据量引起客户端崩溃，该工具限制了单次最大返回行数。".to_string(),
            input_schema: InputSchema {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = HashMap::new();
                    map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                    map.insert("sql".to_string(), Property { prop_type: "string".to_string(), description: "要执行的 SQL 查询语句（必须为只读操作）".to_string() });
                    map.insert("maxRows".to_string(), Property { prop_type: "integer".to_string(), description: "最大返回行数，可选，默认由系统配置决定".to_string() });
                    map
                },
                required: vec!["dataSource".to_string(), "sql".to_string()],
            },
        },
        Tool {
            name: "explain_query".to_string(),
            description: "获取指定 SQL 在目标数据源上的执行计划（EXPLAIN 计划）".to_string(),
            input_schema: InputSchema {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = HashMap::new();
                    map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                    map.insert("sql".to_string(), Property { prop_type: "string".to_string(), description: "待分析的 SQL 查询语句".to_string() });
                    map
                },
                required: vec!["dataSource".to_string(), "sql".to_string()],
            },
        },
        Tool {
            name: "count_rows".to_string(),
            description: "快速统计指定表的数据行数，支持通过 SQL 语法的 where 条件进行过滤（只允许只读条件，杜绝子查询注入）。".to_string(),
            input_schema: InputSchema {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = server.base_table_properties();
                    map.insert("where".to_string(), Property { prop_type: "string".to_string(), description: "过滤条件 (例如 age > 18，不需要带 WHERE 关键字)".to_string() });
                    map
                },
                required: vec!["dataSource".to_string(), "table".to_string()],
            },
        },
        Tool {
            name: "sample_data".to_string(),
            description: "快速获取表的样例数据，默认最多返回 10 行数据。".to_string(),
            input_schema: InputSchema {
                schema_type: "object".to_string(),
                properties: {
                    let mut map = server.base_table_properties();
                    map.insert("limit".to_string(), Property { prop_type: "integer".to_string(), description: "返回的最大样本数，默认 10 行，最大 100 行".to_string() });
                    map
                },
                required: vec!["dataSource".to_string(), "table".to_string()],
            },
        },
    ]
}

impl McpServer {
    pub(crate) async fn handle_execute_query(&self, ds: &str, args: &Value) -> Result<String> {
        let sql = args
            .get("sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing parameter 'sql'"))?;
        let max_rows_param = args
            .get("maxRows")
            .and_then(|v| v.as_i64())
            .map(|n| n as usize);
        let max_rows = max_rows_param.unwrap_or(self.config.mcp.security.max_rows);
        let limit = max_rows.min(self.config.mcp.security.max_rows);

        let conn = self.registry.get_connector(ds)?;
        let query_res = self.execute_and_audit(&conn, ds, sql, limit).await;
        if let Err(ref e) = query_res {
            let err_msg = e.to_string().to_lowercase();
            if (err_msg.contains("no such table")
                || err_msg.contains("no such column")
                || err_msg.contains("table doesn't exist")
                || err_msg.contains("unknown column")
                || err_msg.contains("undefined_table")
                || err_msg.contains("undefined_column"))
                && let Some(ref store) = self.metadata_store
            {
                let _ = store.invalidate_schema_cache(ds).await;
            }
        }
        query_res
    }

    pub(crate) async fn handle_explain_query(&self, ds: &str, args: &Value) -> Result<String> {
        let sql = args
            .get("sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing parameter 'sql'"))?;
        let explain_sql = format!("EXPLAIN {}", sql);
        let conn = self.registry.get_connector(ds)?;
        self.execute_and_audit(&conn, ds, &explain_sql, 100).await
    }

    pub(crate) async fn handle_count_rows(&self, ds: &str, args: &Value) -> Result<String> {
        let (table, db) = self.get_table_and_db(args)?;
        let where_clause = args
            .get("where")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let conn = self.registry.get_connector(ds)?;
        let count = conn.count_rows(db, table, where_clause).await?;
        Ok(count.to_string())
    }

    pub(crate) async fn handle_sample_data(&self, ds: &str, args: &Value) -> Result<String> {
        let (table, db) = self.get_table_and_db(args)?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .map(|n| n as usize)
            .unwrap_or(10)
            .min(100);
        let conn = self.registry.get_connector(ds)?;
        let res = conn.sample_data(db, table, limit).await?;
        Ok(serde_json::to_string_pretty(&res)?)
    }
}
