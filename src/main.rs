mod config;
mod db;
mod mcp;
mod security;

use clap::Parser;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, BufReader};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 配置文件路径
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 安装 sqlx 的 AnyPool 驱动
    sqlx::any::install_default_drivers();

    let args = Args::parse();

    let cfg = match args.config {
        Some(ref path) => {
            if Path::new(path).exists() {
                eprintln!("正在加载配置文件: {}", path);
                config::load_config(path)?
            } else {
                return Err(format!("错误: 找不到指定的配置文件 '{}'", path).into());
            }
        }
        None => {
            if Path::new("application.yml").exists() {
                eprintln!("正在加载默认配置文件: application.yml");
                config::load_config("application.yml")?
            } else {
                eprintln!("未指定配置文件，将使用默认配置启动（无初始数据源）...");
                config::Config::default()
            }
        }
    };

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

    let server = Arc::new(mcp::McpServer::new(cfg, registry));

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
