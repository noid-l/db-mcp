use anyhow::Result;
use serde_json::{Value, json};

use crate::server::McpServer;
use crate::types::Resource;

pub async fn handle_resources_list(_server: &McpServer) -> Result<Value> {
    let resources = vec![Resource {
        uri: "db://dataSources".to_string(),
        name: "dataSources".to_string(),
        mime_type: "text/plain".to_string(),
        description: "当前已配置的所有可用数据源及其类型列表".to_string(),
    }];
    Ok(json!({ "resources": resources }))
}

pub async fn handle_resources_read(server: &McpServer, params: Option<Value>) -> Result<Value> {
    let p = params.ok_or_else(|| anyhow::anyhow!("Missing params"))?;
    let uri = p
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing parameter 'uri'"))?;

    if uri == "db://dataSources" {
        let types = server.registry.get_datasource_types();
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
    if let Some(rem) = uri.strip_prefix("db://") {
        let parts: Vec<&str> = rem.split('/').collect();
        if parts.len() == 3 && parts[1] == "tables" {
            let ds = parts[0];
            let table = parts[2];

            let conn = server.registry.get_connector(ds)?;
            let cols = conn.describe_table(None, table).await?;

            let mut sb = format!("### Table Schema: {}.{}\n\n", ds, table);
            sb.push_str("| Column Name | Type | Nullable | Default | PK | Comment |\n");
            sb.push_str("|-------------|------|----------|---------|----|---------|\n");
            for col in cols {
                let nullable_str = if col.nullable { "YES" } else { "NO" };
                let pk_str = if col.primary_key { "YES" } else { "NO" };
                let def_val = col.default_value.as_deref().unwrap_or("NULL");
                sb.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    col.name, col.type_name, nullable_str, def_val, pk_str, col.comment
                ));
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
