use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use anyhow::Result;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: InputSchema,
}

#[derive(Debug, Serialize, Clone)]
pub struct InputSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: HashMap<String, Property>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Property {
    #[serde(rename = "type")]
    pub prop_type: String,
    pub description: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub description: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Prompt {
    pub name: String,
    pub description: String,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Clone)]
pub struct PromptMessage {
    pub role: String,
    pub content: PromptContent,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Clone)]
pub struct PromptContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

pub struct McpServer {
    config: crate::config::Config,
    registry: Arc<crate::db::DataSourceRegistry>,
    sql_validator: crate::security::SqlValidator,
}

impl McpServer {
    pub fn new(config: crate::config::Config, registry: Arc<crate::db::DataSourceRegistry>) -> Self {
        let security_cfg = config.mcp.security.clone();
        let sql_validator = crate::security::SqlValidator::new(security_cfg.allowed_prefixes.clone());
        Self { config, registry, sql_validator }
    }

    pub fn audit(&self, ds: &str, sql: &str, elapsed_ms: i64, row_count: usize, err: Option<&anyhow::Error>) {
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

        if let Ok(mut file) = OpenOptions::new().create(true).write(true).append(true).open(log_file) {
            let _ = file.write_all(log_line.as_bytes());
        } else {
            eprint!("[AUDIT] {}", log_line);
        }
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id.unwrap_or(Value::Null);

        // 处理 notification
        if id.is_null() {
            return None;
        }

        let result = match req.method.as_str() {
            "initialize" => self.handle_initialize().await,
            "tools/list" => self.handle_tools_list().await,
            "tools/call" => self.handle_tools_call(req.params).await,
            "resources/list" => self.handle_resources_list().await,
            "resources/read" => self.handle_resources_read(req.params).await,
            "prompts/list" => self.handle_prompts_list().await,
            "prompts/get" => self.handle_prompts_get(req.params).await,
            _ => {
                return Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
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
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(res),
                error: None,
            }),
            Err(e) => Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
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

    async fn handle_initialize(&self) -> Result<Value> {
        Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {},
                "resources": {},
                "prompts": {}
            },
            "serverInfo": {
                "name": "db-mcp-rust",
                "version": "1.0.0"
            }
        }))
    }

    async fn handle_tools_list(&self) -> Result<Value> {
        let tools = vec![
            Tool {
                name: "list_dataSources".to_string(),
                description: "列出当前系统中配置的所有可用数据源名称及其对应的数据库类型。这通常是 AI 助手首先调用的工具，用以知晓可以使用哪些数据源。".to_string(),
                input_schema: InputSchema {
                    schema_type: "object".to_string(),
                    properties: HashMap::new(),
                    required: Vec::new(),
                },
            },
            Tool {
                name: "test_connection".to_string(),
                description: "测试指定数据源的物理连接是否畅通。可以用在开始做一系列查询前确认数据库是否可以正常连接。".to_string(),
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
                name: "get_datasource_info".to_string(),
                description: "获取指定数据源数据库的版本等物理属性信息。".to_string(),
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
                name: "list_databases".to_string(),
                description: "列出指定数据源下的所有数据库/Schema 列表。".to_string(),
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
                input_schema: InputSchema {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut map = HashMap::new();
                        map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                        map.insert("table".to_string(), Property { prop_type: "string".to_string(), description: "表名称".to_string() });
                        map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库/Schema 名称，可选".to_string() });
                        map
                    },
                    required: vec!["dataSource".to_string(), "table".to_string()],
                },
            },
            Tool {
                name: "list_indexes".to_string(),
                description: "列出指定表的索引信息，包括索引名称、包含的列名、是否唯一等属性。".to_string(),
                input_schema: InputSchema {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut map = HashMap::new();
                        map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                        map.insert("table".to_string(), Property { prop_type: "string".to_string(), description: "表名称".to_string() });
                        map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库/Schema 名称，可选".to_string() });
                        map
                    },
                    required: vec!["dataSource".to_string(), "table".to_string()],
                },
            },
            Tool {
                name: "get_foreign_keys".to_string(),
                description: "获取指定表的外键关联引用关系，展示主键表、主键列、外键列等关联细节。".to_string(),
                input_schema: InputSchema {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut map = HashMap::new();
                        map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                        map.insert("table".to_string(), Property { prop_type: "string".to_string(), description: "表名称".to_string() });
                        map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库/Schema 名称，可选".to_string() });
                        map
                    },
                    required: vec!["dataSource".to_string(), "table".to_string()],
                },
            },
            Tool {
                name: "get_table_ddl".to_string(),
                description: "获取指定表或视图的 DDL 建表/建视图语句（方言级别自动适配，不支持的数据库会平滑降级）。".to_string(),
                input_schema: InputSchema {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut map = HashMap::new();
                        map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                        map.insert("table".to_string(), Property { prop_type: "string".to_string(), description: "表名称".to_string() });
                        map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库/Schema 名称，可选".to_string() });
                        map
                    },
                    required: vec!["dataSource".to_string(), "table".to_string()],
                },
            },
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
                        let mut map = HashMap::new();
                        map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                        map.insert("table".to_string(), Property { prop_type: "string".to_string(), description: "表名称".to_string() });
                        map.insert("where".to_string(), Property { prop_type: "string".to_string(), description: "过滤条件 (例如 age > 18，不需要带 WHERE 关键字)".to_string() });
                        map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库/Schema 名称，可选".to_string() });
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
                        let mut map = HashMap::new();
                        map.insert("dataSource".to_string(), Property { prop_type: "string".to_string(), description: "数据源名称".to_string() });
                        map.insert("table".to_string(), Property { prop_type: "string".to_string(), description: "表名称".to_string() });
                        map.insert("limit".to_string(), Property { prop_type: "integer".to_string(), description: "返回的最大样本数，默认 10 行，最大 100 行".to_string() });
                        map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库/Schema 名称，可选".to_string() });
                        map
                    },
                    required: vec!["dataSource".to_string(), "table".to_string()],
                },
            },
            Tool {
                name: "add_dataSource".to_string(),
                description: "在运行时动态添加并连接一个新的数据源。".to_string(),
                input_schema: InputSchema {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut map = HashMap::new();
                        map.insert("name".to_string(), Property { prop_type: "string".to_string(), description: "数据源的唯一名称，例如 'my_mysql'".to_string() });
                        map.insert("dbType".to_string(), Property { prop_type: "string".to_string(), description: "数据库类型，支持: MYSQL, POSTGRESQL, SQLITE, SQLSERVER".to_string() });
                        map.insert("dsn".to_string(), Property { prop_type: "string".to_string(), description: "直接指定 Rust 的 DSN 连接字符串，例如 'sqlite::memory:' 或 'mysql://user:pass@host:port/db'。".to_string() });
                        map.insert("host".to_string(), Property { prop_type: "string".to_string(), description: "主机名 (如果未提供 dsn)".to_string() });
                        map.insert("port".to_string(), Property { prop_type: "integer".to_string(), description: "端口号 (如果未提供 dsn)".to_string() });
                        map.insert("database".to_string(), Property { prop_type: "string".to_string(), description: "数据库名或 SQLite 文件路径 (如果未提供 dsn)".to_string() });
                        map.insert("username".to_string(), Property { prop_type: "string".to_string(), description: "用户名 (如果未提供 dsn)".to_string() });
                        map.insert("password".to_string(), Property { prop_type: "string".to_string(), description: "密码 (如果未提供 dsn)".to_string() });
                        map.insert("jdbcUrl".to_string(), Property { prop_type: "string".to_string(), description: "JDBC URL (将自动解析转化为 DSN，如果未提供 dsn 且未提供 host/port 等)".to_string() });
                        map
                    },
                    required: vec!["name".to_string(), "dbType".to_string()],
                },
            },
            Tool {
                name: "remove_dataSource".to_string(),
                description: "从数据源管理器中移除已存在的数据源连接。".to_string(),
                input_schema: InputSchema {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut map = HashMap::new();
                        map.insert("name".to_string(), Property { prop_type: "string".to_string(), description: "要移除的数据源的名称".to_string() });
                        map
                    },
                    required: vec!["name".to_string()],
                },
            },
        ];
        Ok(json!({ "tools": tools }))
    }

    async fn handle_tools_call(&self, params: Option<Value>) -> Result<Value> {
        let p = params.ok_or_else(|| anyhow::anyhow!("Missing params"))?;
        let name = p.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
        let arguments = p.get("arguments").ok_or_else(|| anyhow::anyhow!("Missing parameter 'arguments'"))?;

        let (content, is_error) = self.call_tool(name, arguments).await;
        Ok(json!({
            "content": [{ "type": "text", "text": content }],
            "isError": is_error
        }))
    }

    async fn call_tool(&self, name: &str, args: &Value) -> (String, bool) {
        let ds = match args.get("dataSource").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                if name == "list_dataSources" || name == "add_dataSource" || name == "remove_dataSource" { "" } else { return ("Missing parameter 'dataSource'".to_string(), true); }
            }
        };

        let result: Result<String> = async {
            match name {
                "list_dataSources" => {
                    Ok(serde_json::to_string_pretty(&self.registry.get_datasource_types())?)
                }
                "add_dataSource" => {
                    let ds_name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
                    let db_type = args.get("dbType").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'dbType'"))?;
                    
                    let dsn = args.get("dsn").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let jdbc_url = args.get("jdbcUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let host = args.get("host").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let port = args.get("port").and_then(|v| v.as_u64()).map(|n| n as u16);
                    let database = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let username = args.get("username").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let password = args.get("password").and_then(|v| v.as_str()).map(|s| s.to_string());
                    
                    let mut properties = None;
                    if let Some(props_val) = args.get("properties") {
                        if let Some(obj) = props_val.as_object() {
                            let mut map = HashMap::new();
                            for (k, v) in obj {
                                if let Some(s) = v.as_str() {
                                    map.insert(k.clone(), s.to_string());
                                }
                            }
                            properties = Some(map);
                        }
                    }

                    let ds_cfg = crate::config::DataSourceConfig {
                        db_type: db_type.to_string(),
                        jdbc_url,
                        host,
                        port,
                        database,
                        username,
                        password,
                        dsn,
                        properties,
                    };

                    let security_cfg = &self.config.mcp.security;
                    
                    let connector = crate::db::BaseConnector::new(
                        db_type,
                        ds_cfg,
                        security_cfg.query_timeout,
                        security_cfg.max_result_set_size_bytes,
                        self.sql_validator.clone()
                    ).await?;

                    // 测试连接
                    if !connector.test_connection().await {
                        return Err(anyhow::anyhow!("Failed to establish a connection to datasource '{}'", ds_name));
                    }

                    self.registry.register(ds_name.to_string(), connector);
                    Ok(format!("Successfully registered data source '{}'", ds_name))
                }
                "remove_dataSource" => {
                    let ds_name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
                    if self.registry.unregister(ds_name) {
                        Ok(format!("Successfully removed data source '{}'", ds_name))
                    } else {
                        Err(anyhow::anyhow!("Datasource '{}' not found", ds_name))
                    }
                }
                "test_connection" => {
                    let conn = self.registry.get_connector(ds)?;
                    let connected = conn.test_connection().await;
                    Ok(serde_json::to_string_pretty(&connected)?)
                }
                "get_datasource_info" => {
                    let conn = self.registry.get_connector(ds)?;
                    Ok(format!("Type: {} | Version: {}", conn.db_type(), conn.get_version()))
                }
                "list_databases" => {
                    let conn = self.registry.get_connector(ds)?;
                    let dbs = conn.list_databases().await?;
                    Ok(serde_json::to_string_pretty(&dbs)?)
                }
                "list_tables" => {
                    let db = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let pattern = args.get("pattern").and_then(|v| v.as_str());
                    let conn = self.registry.get_connector(ds)?;
                    let mut tables = conn.list_tables(db).await?;
                    if let Some(pat) = pattern {
                        let regex_str = crate::config::wildcard_to_regex(pat);
                        if let Ok(reg) = regex::Regex::new(&format!("(?i){}", regex_str)) {
                            tables.retain(|t| reg.is_match(&t.name));
                        }
                    }
                    Ok(serde_json::to_string_pretty(&tables)?)
                }
                "describe_table" => {
                    let table = args.get("table").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing table"))?;
                    let db = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let conn = self.registry.get_connector(ds)?;
                    let cols = conn.describe_table(db, table).await?;
                    Ok(serde_json::to_string_pretty(&cols)?)
                }
                "list_indexes" => {
                    let table = args.get("table").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing table"))?;
                    let db = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let conn = self.registry.get_connector(ds)?;
                    let idxs = conn.list_indexes(db, table).await?;
                    Ok(serde_json::to_string_pretty(&idxs)?)
                }
                "get_foreign_keys" => {
                    let table = args.get("table").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing table"))?;
                    let db = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let conn = self.registry.get_connector(ds)?;
                    let fkeys = conn.get_imported_keys(db, table).await?;
                    Ok(serde_json::to_string_pretty(&fkeys)?)
                }
                "get_table_ddl" => {
                    let table = args.get("table").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing table"))?;
                    let db = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let conn = self.registry.get_connector(ds)?;
                    let ddl = conn.get_table_ddl(db, table).await?;
                    Ok(ddl)
                }
                "execute_query" => {
                    let sql = args.get("sql").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'sql'"))?;
                    let max_rows_param = args.get("maxRows").and_then(|v| v.as_i64()).map(|n| n as usize);
                    let max_rows = max_rows_param.unwrap_or(self.config.mcp.security.max_rows);
                    let limit = max_rows.min(self.config.mcp.security.max_rows);

                    let conn = self.registry.get_connector(ds)?;
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
                "explain_query" => {
                    let sql = args.get("sql").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'sql'"))?;
                    let explain_sql = format!("EXPLAIN {}", sql);
                    let conn = self.registry.get_connector(ds)?;
                    
                    let start = Instant::now();
                    let res = conn.execute_query(&explain_sql, 100).await;
                    let elapsed = start.elapsed().as_millis() as i64;

                    match &res {
                        Ok(qr) => {
                            self.audit(ds, &explain_sql, elapsed, qr.row_count, None);
                        }
                        Err(e) => {
                            self.audit(ds, &explain_sql, elapsed, 0, Some(e));
                        }
                    }
                    Ok(serde_json::to_string_pretty(&res?)?)
                }
                "count_rows" => {
                    let table = args.get("table").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing table"))?;
                    let db = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let where_clause = args.get("where").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let conn = self.registry.get_connector(ds)?;
                    let count = conn.count_rows(db, table, where_clause).await?;
                    Ok(count.to_string())
                }
                "sample_data" => {
                    let table = args.get("table").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing table"))?;
                    let db = args.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|n| n as usize).unwrap_or(10).min(100);
                    let conn = self.registry.get_connector(ds)?;
                    let res = conn.sample_data(db, table, limit).await?;
                    Ok(serde_json::to_string_pretty(&res)?)
                }
                _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
            }
        }.await;

        match result {
            Ok(content) => (content, false),
            Err(e) => (e.to_string(), true),
        }
    }

    async fn handle_resources_list(&self) -> Result<Value> {
        let resources = vec![
            Resource {
                uri: "db://dataSources".to_string(),
                name: "dataSources".to_string(),
                mime_type: "text/plain".to_string(),
                description: "当前已配置的所有可用数据源及其类型列表".to_string(),
            }
        ];
        Ok(json!({ "resources": resources }))
    }

    async fn handle_resources_read(&self, params: Option<Value>) -> Result<Value> {
        let p = params.ok_or_else(|| anyhow::anyhow!("Missing params"))?;
        let uri = p.get("uri").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'uri'"))?;

        if uri == "db://dataSources" {
            let types = self.registry.get_datasource_types();
            let mut sb = String::from("=== Available DataSources ===\n");
            for (name, t) in types {
                sb.push_str(&format!("- {} (Type: {})\n", name, t));
            }
            return Ok(json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": "text/plain",
                    "text": sb
                }]
            }));
        }

        // 匹配 db://{dataSource}/tables/{table}
        if uri.starts_with("db://") {
            let rem = &uri[5..];
            let parts: Vec<&str> = rem.split('/').collect();
            if parts.len() == 3 && parts[1] == "tables" {
                let ds = parts[0];
                let table = parts[2];

                let conn = self.registry.get_connector(ds)?;
                let cols = conn.describe_table(None, table).await?;

                let mut sb = format!("### Table Schema: {}.{}\n\n", ds, table);
                sb.push_str("| Column Name | Type | Nullable | Default | PK | Comment |\n");
                sb.push_str("|-------------|------|----------|---------|----|---------|\n");
                for col in cols {
                    let nullable_str = if col.nullable { "YES" } else { "NO" };
                    let pk_str = if col.primary_key { "YES" } else { "NO" };
                    let def_val = col.default_value.as_deref().unwrap_or("NULL");
                    sb.push_str(&format!("| {} | {} | {} | {} | {} | {} |\n",
                        col.name, col.type_name, nullable_str, def_val, pk_str, col.comment));
                }

                return Ok(json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "text/plain",
                        "text": sb
                    }]
                }));
            }
        }

        Err(anyhow::anyhow!("Unknown resource URI: {}", uri))
    }

    async fn handle_prompts_list(&self) -> Result<Value> {
        let prompts = vec![
            Prompt {
                name: "query_builder".to_string(),
                description: "根据您对查询需求的自然语言描述，结合目标数据源的表结构上下文，生成只读 SQL 查询语句。".to_string(),
            }
        ];
        Ok(json!({ "prompts": prompts }))
    }

    async fn handle_prompts_get(&self, params: Option<Value>) -> Result<Value> {
        let p = params.ok_or_else(|| anyhow::anyhow!("Missing params"))?;
        let name = p.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
        let arguments = p.get("arguments").ok_or_else(|| anyhow::anyhow!("Missing parameter 'arguments'"))?;

        if name == "query_builder" {
            let ds = arguments.get("dataSource").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing argument 'dataSource'"))?;
            let desc = arguments.get("description").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Missing argument 'description'"))?;

            let mut schema_ctx = String::new();
            if let Ok(conn) = self.registry.get_connector(ds) {
                if let Ok(tables) = conn.list_tables(None).await {
                    schema_ctx.push_str(&format!("目标数据源 [{}] 中可用的表列表如下：\n", ds));
                    for t in tables {
                        let comment = if t.comment.is_empty() { "无" } else { &t.comment };
                        schema_ctx.push_str(&format!("- 表名: `{}` | 类型: {} | 注释: {}\n", t.name, t.table_type, comment));
                    }
                } else {
                    schema_ctx.push_str(&format!("无法获取数据源 [{}] 的表列表元数据，请仅根据通用 SQL 生成。\n", ds));
                }
            } else {
                schema_ctx.push_str(&format!("无法获取数据源 [{}] 的表列表元数据，请仅根据通用 SQL 生成。\n", ds));
            }

            let system_instruction = format!(
                "你是一个专业的数据库专家和 SQL 生成助手。\n\
                 请严格根据以下上下文信息，为用户编写语法正确的只读 SQL (SELECT) 语句。\n\
                 注意：你只能生成 SELECT 语句，不能生成任何 INSERT/UPDATE/DELETE/DROP 等写入操作。\n\n\
                 {}",
                schema_ctx
            );

            let user_request = format!(
                "请为我生成满足以下需求的 SQL 查询：\n\n{}\n\n请直接给出 SQL 代码，并提供必要的表结构关联和注释说明。",
                desc
            );

            return Ok(json!({
                "description": "自然语言转 SQL 查询向导",
                "messages": [
                    {
                        "role": "user",
                        "content": { "type": "text", "text": system_instruction }
                    },
                    {
                        "role": "user",
                        "content": { "type": "text", "text": user_request }
                    }
                ]
            }));
        }

        Err(anyhow::anyhow!("Unknown prompt: {}", name))
    }
}
