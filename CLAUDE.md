# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

db-mcp 是一个基于 Rust 的 MCP (Model Context Protocol) 服务端，用于连接 LLM 与关系型数据库，提供安全、只读的数据库查询能力。通过 STDIO 上的 JSON-RPC 协议与 MCP 客户端（如 Claude Desktop、Cursor）通信。

## 常用命令

```bash
# 构建
cargo build --release          # 产物: target/release/db-mcp

# 检查 & lint
cargo check
cargo clippy -- -D warnings    # CI 中严格模式，所有 warning 视为错误
cargo fmt --all -- --check     # 格式检查

# 测试
cargo test                     # 运行全部测试
cargo test -p db-core          # 运行单个 crate 的测试
cargo test test_metadata_store # 运行单个测试函数

# 运行
./target/release/db-mcp        # 启动服务，监听 STDIO
```

## 架构

Rust workspace，三个 crate，依赖关系为 `cli → mcp-server → db-core`：

```
crates/
├── db-core/          # 共享库：数据库连接、SQL 安全校验、元数据缓存
│   └── src/
│       ├── config.rs       # 配置结构体、DSN 转换、SQL 规范化、通配符转换
│       ├── security.rs     # SQL 白名单前缀校验器
│       ├── db.rs           # BaseConnector（连接池+查询执行+行数/大小截断）、DataSourceRegistry
│       ├── db/
│       │   ├── dialect.rs  # DbDialect trait（数据库方言抽象层）
│       │   ├── mysql.rs    # MySQL/MariaDB/GBASE 实现
│       │   ├── postgres.rs # PostgreSQL/KingBase/HighGo 实现
│       │   ├── sqlite.rs   # SQLite 实现
│       │   └── sqlserver.rs# SQL Server 实现
│       └── metadata.rs     # MetadataStore（SQLite 元数据库：数据源持久化 + Schema 缓存）
├── mcp-server/       # MCP 协议层：JSON-RPC 处理、工具定义与路由
│   └── src/lib.rs          # McpServer：请求分发、16 个 MCP 工具、审计日志、资源/Prompt
└── cli/              # 可执行入口
    └── src/main.rs         # 初始化驱动 → 加载配置 → 还原缓存数据源 → STDIO JSON-RPC 循环
```

## 关键设计

- **方言模式 (DbDialect trait)**：`db-core/src/db/dialect.rs` 定义了统一的数据库操作接口，每个数据库类型实现自己的 SQL 方言（表名格式化、LIMIT 追加、元数据查询等）。添加新数据库支持需要实现该 trait 并在 `get_dialect()` 中注册。

- **安全机制**：`SqlValidator` 仅允许 `SELECT/SHOW/DESC/EXPLAIN` 前缀；`BaseConnector::execute_query` 自动追加 LIMIT 并在运行时按行数（默认 1000）和结果集大小（默认 10MB）截断；查询超时（默认 30s）。

- **元数据持久化**：`MetadataStore` 使用本地 SQLite 文件 `mcp_meta.db` 存储已注册的数据源配置和 Schema 缓存，支持按指纹比对和 TTL（4小时）的缓存失效策略。查询失败时自动失效相关缓存。

- **MCP 协议**：所有工具在 `mcp-server/src/lib.rs` 的 `handle_tools_list` 中定义，在 `call_tool` 中路由处理。InputSchema 定义复用了 `base_table_properties()`、`table_schema()`、`datasource_only_schema()` 三个辅助方法。

## 数据库连接格式

支持三种连接方式（优先级：DSN > JDBC URL > Host/Port 表单），见 `config::convert_to_dsn`。JDBC URL 自动转换为 sqlx 支持的 DSN 格式。

## 注意事项

- Rust edition 2024，需要 Rust 1.75+
- 本项目无配置文件，默认值硬编码在 `config.rs` 的 `Default` 实现中
- sqlx 使用 `runtime-tokio-rustls`，不含 `tiberius`（SQL Server 通过连接字符串支持）
- 项目中的注释和工具描述为中文
