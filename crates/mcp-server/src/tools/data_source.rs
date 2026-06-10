use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

use crate::server::McpServer;
use crate::types::{InputSchema, Property, Tool};

pub fn tool_definitions() -> Vec<Tool> {
    vec![
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
    ]
}

impl McpServer {
    pub(crate) async fn handle_list_data_sources(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(
            &self.registry.get_datasource_types(),
        )?)
    }

    pub(crate) async fn handle_add_data_source(&self, args: &Value) -> Result<String> {
        let ds_name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
        let db_type = args
            .get("dbType")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing parameter 'dbType'"))?;

        let dsn = args
            .get("dsn")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let jdbc_url = args
            .get("jdbcUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let host = args
            .get("host")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let port = args.get("port").and_then(|v| v.as_u64()).map(|n| n as u16);
        let database = args
            .get("database")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let username = args
            .get("username")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let password = args
            .get("password")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut properties = None;
        if let Some(obj) = args.get("properties").and_then(|v| v.as_object()) {
            let mut map = HashMap::new();
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    map.insert(k.clone(), s.to_string());
                }
            }
            properties = Some(map);
        }

        let ds_cfg = db_core::config::DataSourceConfig {
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

        let connector = db_core::db::BaseConnector::new(
            db_type,
            ds_cfg.clone(),
            security_cfg.query_timeout,
            security_cfg.max_result_set_size_bytes,
            self.sql_validator.clone(),
        )
        .await?;

        // 测试连接
        if !connector.test_connection().await {
            return Err(anyhow::anyhow!(
                "Failed to establish a connection to datasource '{}'",
                ds_name
            ));
        }

        self.registry.register(ds_name.to_string(), connector);
        if let Some(ref store) = self.metadata_store
            && let Err(e) = store.save_data_source(ds_name, db_type, &ds_cfg).await
        {
            eprintln!("警告: 缓存数据源 '{}' 到本地数据库失败: {}", ds_name, e);
        }
        Ok(format!("Successfully registered data source '{}'", ds_name))
    }

    pub(crate) async fn handle_remove_data_source(&self, args: &Value) -> Result<String> {
        let ds_name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
        if self.registry.unregister(ds_name) {
            if let Some(ref store) = self.metadata_store
                && let Err(e) = store.remove_data_source(ds_name).await
            {
                eprintln!("警告: 从本地数据库删除数据源 '{}' 缓存失败: {}", ds_name, e);
            }
            Ok(format!("Successfully removed data source '{}'", ds_name))
        } else {
            Err(anyhow::anyhow!("Datasource '{}' not found", ds_name))
        }
    }
}
