pub mod cache;
pub mod data_source;
pub mod query;
pub mod schema;

use anyhow::Result;
use serde_json::{Value, json};

use crate::server::McpServer;

/// initialize 响应
pub async fn handle_initialize() -> Result<Value> {
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

/// 聚合所有工具定义
pub async fn handle_tools_list(server: &McpServer) -> Result<Value> {
    let mut tools = Vec::new();
    tools.extend(data_source::tool_definitions());
    tools.extend(schema::tool_definitions(server));
    tools.extend(query::tool_definitions(server));
    tools.extend(cache::tool_definitions());
    // 以下工具定义复用 server 的 helper 方法
    tools.push(crate::types::Tool {
        name: "test_connection".to_string(),
        description: "测试指定数据源的物理连接是否畅通。可以用在开始做一系列查询前确认数据库是否可以正常连接。".to_string(),
        input_schema: server.datasource_only_schema(),
    });
    tools.push(crate::types::Tool {
        name: "get_datasource_info".to_string(),
        description: "获取指定数据源数据库的版本等物理属性信息。".to_string(),
        input_schema: server.datasource_only_schema(),
    });
    tools.push(crate::types::Tool {
        name: "list_databases".to_string(),
        description: "列出指定数据源下的所有数据库/Schema 列表。".to_string(),
        input_schema: server.datasource_only_schema(),
    });
    Ok(json!({ "tools": tools }))
}

/// tools/call 处理：参数解析 + 调度到具体工具处理方法
pub async fn handle_tools_call(server: &McpServer, params: Option<Value>) -> Result<Value> {
    let p = params.ok_or_else(|| anyhow::anyhow!("Missing params"))?;
    let name = p
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
    let arguments = p
        .get("arguments")
        .ok_or_else(|| anyhow::anyhow!("Missing parameter 'arguments'"))?;

    let (content, is_error) = call_tool(server, name, arguments).await;
    Ok(json!({
        "content": [{ "type": "text", "text": content }],
        "isError": is_error
    }))
}

async fn call_tool(server: &McpServer, name: &str, args: &Value) -> (String, bool) {
    let ds = match args.get("dataSource").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            if name == "list_dataSources" || name == "add_dataSource" || name == "remove_dataSource"
            {
                ""
            } else {
                return ("Missing parameter 'dataSource'".to_string(), true);
            }
        }
    };

    let result: Result<String> = async {
        match name {
            // 数据源管理
            "list_dataSources" => server.handle_list_data_sources().await,
            "add_dataSource" => server.handle_add_data_source(args).await,
            "remove_dataSource" => server.handle_remove_data_source(args).await,
            // 连接 & 信息
            "test_connection" => {
                let conn = server.registry.get_connector(ds)?;
                let connected = conn.test_connection().await;
                Ok(serde_json::to_string_pretty(&connected)?)
            }
            "get_datasource_info" => {
                let conn = server.registry.get_connector(ds)?;
                Ok(format!(
                    "Type: {} | Version: {}",
                    conn.db_type(),
                    conn.get_version()
                ))
            }
            "list_databases" => {
                let conn = server.registry.get_connector(ds)?;
                let dbs = conn.list_databases().await?;
                Ok(serde_json::to_string_pretty(&dbs)?)
            }
            // Schema 发现
            "list_tables" => server.handle_list_tables(ds, args).await,
            "describe_table" => server.handle_describe_table(ds, args).await,
            "list_indexes" => server.handle_list_indexes(ds, args).await,
            "get_foreign_keys" => server.handle_get_foreign_keys(ds, args).await,
            "get_table_ddl" => server.handle_get_table_ddl(ds, args).await,
            // 查询执行
            "execute_query" => server.handle_execute_query(ds, args).await,
            "explain_query" => server.handle_explain_query(ds, args).await,
            "count_rows" => server.handle_count_rows(ds, args).await,
            "sample_data" => server.handle_sample_data(ds, args).await,
            // 缓存管理
            "refresh_schema" => server.handle_refresh_schema(ds, args).await,
            "search_schema" => server.handle_search_schema(ds, args).await,
            _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
        }
    }
    .await;

    match result {
        Ok(content) => (content, false),
        Err(e) => (e.to_string(), true),
    }
}
