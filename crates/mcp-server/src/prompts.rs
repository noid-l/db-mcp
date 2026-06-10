use anyhow::Result;
use serde_json::{Value, json};

use crate::server::McpServer;
use crate::types::Prompt;

pub async fn handle_prompts_list(_server: &McpServer) -> Result<Value> {
    let prompts = vec![Prompt {
        name: "query_builder".to_string(),
        description:
            "根据您对查询需求的自然语言描述，结合目标数据源的表结构上下文，生成只读 SQL 查询语句。"
                .to_string(),
    }];
    Ok(json!({ "prompts": prompts }))
}

pub async fn handle_prompts_get(server: &McpServer, params: Option<Value>) -> Result<Value> {
    let p = params.ok_or_else(|| anyhow::anyhow!("Missing params"))?;
    let name = p
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing parameter 'name'"))?;
    let arguments = p
        .get("arguments")
        .ok_or_else(|| anyhow::anyhow!("Missing parameter 'arguments'"))?;

    if name == "query_builder" {
        let ds = arguments
            .get("dataSource")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing argument 'dataSource'"))?;
        let desc = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing argument 'description'"))?;

        let mut schema_ctx = String::new();
        if let Ok(conn) = server.registry.get_connector(ds) {
            if let Ok(tables) = conn.list_tables(None).await {
                schema_ctx.push_str(&format!("目标数据源 [{}] 中可用的表列表如下：\n", ds));
                for t in tables {
                    let comment = if t.comment.is_empty() {
                        "无"
                    } else {
                        &t.comment
                    };
                    schema_ctx.push_str(&format!(
                        "- 表名: `{}` | 类型: {} | 注释: {}\n",
                        t.name, t.table_type, comment
                    ));
                }
            } else {
                schema_ctx.push_str(&format!(
                    "无法获取数据源 [{}] 的表列表元数据，请仅根据通用 SQL 生成。\n",
                    ds
                ));
            }
        } else {
            schema_ctx.push_str(&format!(
                "无法获取数据源 [{}] 的表列表元数据，请仅根据通用 SQL 生成。\n",
                ds
            ));
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
