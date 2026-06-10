use clap::Parser;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, BufReader};

use db_core::{config, db, security};
use mcp_server as mcp;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 安装 sqlx 的 AnyPool 驱动
    sqlx::any::install_default_drivers();

    let _args = Args::parse();

    let cfg = config::Config::default();

    eprintln!("正在初始化 SQL 校验器和数据源...");
    let security_cfg = cfg.mcp.security.clone();
    let sql_validator = security::SqlValidator::new(security_cfg.allowed_prefixes.clone());

    let registry = Arc::new(db::DataSourceRegistry::new());

    for (name, ds_cfg) in &cfg.mcp.data_sources {
        eprintln!("正在初始化数据源 '{}' (类型: {})...", name, ds_cfg.db_type);
        match db::BaseConnector::new(
            &ds_cfg.db_type,
            ds_cfg.clone(),
            security_cfg.query_timeout,
            security_cfg.max_result_set_size_bytes,
            sql_validator.clone(),
        )
        .await
        {
            Ok(connector) => {
                // 测试连接
                let connected = connector.test_connection().await;
                if connected {
                    eprintln!("数据源 '{}' 连接测试成功！", name);
                } else {
                    eprintln!("警告: 数据源 '{}' 连接测试失败，但仍将被注册。", name);
                }
                registry.register(name.clone(), connector);
            }
            Err(e) => {
                eprintln!("警告: 数据源 '{}' 初始化失败: {}", name, e);
            }
        }
    }

    let meta_db_path = "mcp_meta.db";
    let metadata_store = Arc::new(db_core::metadata::MetadataStore::new(meta_db_path).await?);

    eprintln!("正在从元数据库还原缓存的数据源...");
    match metadata_store.list_data_sources().await {
        Ok(sources) => {
            for (name, db_type, ds_cfg) in sources {
                eprintln!("正在还原缓存数据源 '{}' (类型: {})...", name, db_type);
                match db::BaseConnector::new(
                    &db_type,
                    ds_cfg.clone(),
                    security_cfg.query_timeout,
                    security_cfg.max_result_set_size_bytes,
                    sql_validator.clone(),
                )
                .await
                {
                    Ok(connector) => {
                        let connected = connector.test_connection().await;
                        if connected {
                            eprintln!("数据源 '{}' 连接还原成功！", name);
                        } else {
                            eprintln!("警告: 数据源 '{}' 还原失败，物理连接不通，但仍将被注册。", name);
                        }
                        registry.register(name, connector);
                    }
                    Err(e) => {
                        eprintln!("警告: 数据源 '{}' 还原连接池失败: {}", name, e);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("警告: 无法加载缓存的数据源配置: {}", e);
        }
    }

    let server = Arc::new(mcp::McpServer::new(cfg, registry, Some(metadata_store)));

    eprintln!("db-mcp 服务初始化完成，开始在 STDIO 上监听 JSON-RPC 请求...");

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                // EOF
                eprintln!("读取到 EOF，服务正在退出...");
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // 反序列化请求
                let req: mcp::JsonRpcRequest = match serde_json::from_str(trimmed) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("解析 JSON-RPC 请求失败: '{}', 错误: {}", trimmed, e);
                        // 返回一个 Parse error
                        let resp = mcp::JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: serde_json::Value::Null,
                            result: None,
                            error: Some(mcp::JsonRpcError {
                                code: -32700,
                                message: format!("Parse error: {}", e),
                                data: None,
                            }),
                        };
                        if let Ok(resp_str) = serde_json::to_string(&resp) {
                            use tokio::io::AsyncWriteExt;
                            let _ = stdout.write_all(resp_str.as_bytes()).await;
                            let _ = stdout.write_all(b"\n").await;
                            let _ = stdout.flush().await;
                        }
                        continue;
                    }
                };

                if let Some(resp) = server.handle_request(req).await {
                    match serde_json::to_string(&resp) {
                        Ok(resp_str) => {
                            use tokio::io::AsyncWriteExt;
                            if let Err(e) = stdout.write_all(resp_str.as_bytes()).await {
                                eprintln!("写入 stdout 错误: {}", e);
                                break;
                            }
                            if let Err(e) = stdout.write_all(b"\n").await {
                                eprintln!("写入 stdout 换行错误: {}", e);
                                break;
                            }
                            if let Err(e) = stdout.flush().await {
                                eprintln!("Flush stdout 错误: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("序列化响应失败: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("读取 stdin 错误: {}", e);
                break;
            }
        }
    }

    Ok(())
}
