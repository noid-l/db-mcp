use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use crate::types::{InputSchema, Property};

pub struct McpServer {
    pub(crate) config: db_core::config::Config,
    pub(crate) registry: Arc<db_core::db::DataSourceRegistry>,
    pub(crate) sql_validator: db_core::security::SqlValidator,
    pub(crate) metadata_store: Option<Arc<db_core::metadata::MetadataStore>>,
}

impl McpServer {
    pub fn new(
        config: db_core::config::Config,
        registry: Arc<db_core::db::DataSourceRegistry>,
        metadata_store: Option<Arc<db_core::metadata::MetadataStore>>,
    ) -> Self {
        let security_cfg = config.mcp.security.clone();
        let sql_validator =
            db_core::security::SqlValidator::new(security_cfg.allowed_prefixes.clone());
        Self {
            config,
            registry,
            sql_validator,
            metadata_store,
        }
    }

    pub(crate) fn audit(
        &self,
        ds: &str,
        sql: &str,
        elapsed_ms: i64,
        row_count: usize,
        err: Option<&anyhow::Error>,
    ) {
        if !self.config.mcp.audit.enabled {
            return;
        }

        let log_file = &self.config.mcp.audit.log_file;
        if let Some(parent) = Path::new(log_file).parent() {
            let _ = fs::create_dir_all(parent);
        }

        let status = if err.is_some() { "FAILED" } else { "SUCCESS" };
        let err_msg = err.map(|e| format!(" | Error: {}", e)).unwrap_or_default();

        let log_line = format!(
            "[{}] DataSource: {} | Time: {}ms | Rows: {} | Status: {}{} | SQL: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            ds,
            elapsed_ms,
            row_count,
            status,
            err_msg,
            sql
        );

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_file) {
            let _ = file.write_all(log_line.as_bytes());
        } else {
            eprint!("[AUDIT] {}", log_line);
        }
    }

    pub(crate) fn base_table_properties(&self) -> HashMap<String, Property> {
        let mut map = HashMap::new();
        map.insert(
            "dataSource".to_string(),
            Property {
                prop_type: "string".to_string(),
                description: "数据源名称".to_string(),
            },
        );
        map.insert(
            "table".to_string(),
            Property {
                prop_type: "string".to_string(),
                description: "表名称".to_string(),
            },
        );
        map.insert(
            "database".to_string(),
            Property {
                prop_type: "string".to_string(),
                description: "数据库/Schema 名称，可选".to_string(),
            },
        );
        map
    }

    pub(crate) fn table_schema(&self) -> InputSchema {
        InputSchema {
            schema_type: "object".to_string(),
            properties: self.base_table_properties(),
            required: vec!["dataSource".to_string(), "table".to_string()],
        }
    }

    pub(crate) fn datasource_only_schema(&self) -> InputSchema {
        let mut map = HashMap::new();
        map.insert(
            "dataSource".to_string(),
            Property {
                prop_type: "string".to_string(),
                description: "数据源名称".to_string(),
            },
        );
        InputSchema {
            schema_type: "object".to_string(),
            properties: map,
            required: vec!["dataSource".to_string()],
        }
    }

    pub(crate) fn get_table_and_db<'a>(
        &self,
        args: &'a Value,
    ) -> Result<(&'a str, Option<String>)> {
        let table = args
            .get("table")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing table"))?;
        let db = args
            .get("database")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Ok((table, db))
    }

    pub(crate) async fn execute_and_audit(
        &self,
        conn: &db_core::db::BaseConnector,
        ds: &str,
        sql: &str,
        limit: usize,
    ) -> Result<String> {
        let start = Instant::now();
        let res = conn.execute_query(sql, limit).await;
        let elapsed = start.elapsed().as_millis() as i64;

        match &res {
            Ok(qr) => {
                self.audit(ds, sql, elapsed, qr.row_count, None);
            }
            Err(e) => {
                self.audit(ds, sql, elapsed, 0, Some(e));
            }
        }
        Ok(serde_json::to_string_pretty(&res?)?)
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id.unwrap_or(Value::Null);

        // 处理 notification
        if id.is_null() {
            return None;
        }

        let result = match req.method.as_str() {
            "initialize" => crate::tools::handle_initialize().await,
            "tools/list" => crate::tools::handle_tools_list(self).await,
            "tools/call" => crate::tools::handle_tools_call(self, req.params).await,
            "resources/list" => crate::resources::handle_resources_list(self).await,
            "resources/read" => crate::resources::handle_resources_read(self, req.params).await,
            "prompts/list" => crate::prompts::handle_prompts_list(self).await,
            "prompts/get" => crate::prompts::handle_prompts_get(self, req.params).await,
            _ => {
                return Some(JsonRpcResponse {
                    json_rpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Method not found: {}", req.method),
                        data: None,
                    }),
                });
            }
        };

        match result {
            Ok(res) => Some(JsonRpcResponse {
                json_rpc: "2.0".to_string(),
                id,
                result: Some(res),
                error: None,
            }),
            Err(e) => Some(JsonRpcResponse {
                json_rpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32603,
                    message: e.to_string(),
                    data: None,
                }),
            }),
        }
    }
}
