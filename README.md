# db-mcp

`db-mcp` 是一个基于 Rust 语言开发的 **MCP (Model Context Protocol)** 服务端。它旨在连接大语言模型（LLM）与你的关系型数据库，赋予大模型以安全、可控、只读的方式查询、分析及管理多种关系型数据库的能力。

## 🌟 核心特性

- **多数据库驱动适配**：底层通过 `sqlx` 驱动，完美兼容 **MySQL / MariaDB**, **PostgreSQL**, **SQLite** 以及 **SQL Server**。
- **零配置开箱即用**：删除了对 `application.yml` 配置文件的强依赖。无需提前配置任何数据库即可一键启动服务，随后在运行时动态建立连接。
- **动态数据源管理**：
  - 运行时动态添加数据源：支持直连 DSN、JDBC URL 自动转换以及 host/port 表单属性连接。
  - 运行时动态注销数据源：支持断开并销毁现有的连接池释放资源。
- **极致安全校验（防 SQL 注入）**：
  - 内置基于正则的 SQL 词法验证器，仅允许执行只读操作（如 `SELECT`、`SHOW`、`DESC`、`EXPLAIN` 等）。
  - 拦截任何包含破坏性关键字（如 `DROP`、`DELETE`、`UPDATE`、`INSERT`、`TRUNCATE`、`ALTER` 等）的请求。
  - 防止利用子查询或联合查询绕过检测。
- **高可用与防崩溃保护**：
  - **单次最大行数限制**：自动改写并限制 SQL 查询的最大返回行数（默认 1000 行），防止因拉取百万级海量数据导致内存溢出崩溃。
  - **结果集大小限制**：动态计算返回的数据流字节数，超出最大安全阀值（如 10MB）时自动截断。
  - **查询超时机制**：对慢查询进行秒级超时熔断保护，防止请求挂起堆积。
- **完备的审计日志**：
  - 自动记录每次数据库操作的时间、所用数据源、执行耗时、影响行数、成功状态以及完整 SQL 语句至指定审计文件，确保行为可追溯。

---

## 🚀 快速开始

### 1. 编译构建
确保本地已安装 Rust (1.75+) 环境，在项目根目录下执行：
```bash
cargo build --release
```
编译产物将生成在 `target/release/db-mcp`。

### 2. 运行服务
你可以直接以零依赖的纯净模式启动服务：
```bash
./target/release/db-mcp
```
或者，也可以在启动时传入配置文件来初始化预置的数据源：
```bash
./target/release/db-mcp --config application.yml
```

### 3. 配置参考
如需配置预置数据源、安全参数或审计路径，可参考当前目录下的 [application-example.yml](./application-example.yml)。

---

## 🛠️ MCP 工具接口 (Tools)

大语言模型（如 Claude Desktop、Cursor 等 MCP 客户端）连接后，可使用以下暴露的工具：

| 工具名称 | 描述说明 | 核心输入参数 |
| :--- | :--- | :--- |
| **`list_dataSources`** | 列出当前系统中已注册的所有可用数据源名称及数据库类型。 | 无 |
| **`add_dataSource`** | 运行时动态创建、测试并注册新数据库。支持 DSN、JDBC URL 或独立参数。 | `name`, `dbType` |
| **`remove_dataSource`** | 运行时注销并释放已有的数据源连接。 | `name` |
| **`test_connection`** | 测试指定数据源的物理连接是否畅通。 | `dataSource` |
| **`get_datasource_info`**| 获取指定数据源数据库的版本等物理属性信息。 | `dataSource` |
| **`list_databases`** | 列出指定数据源下的所有数据库/Schema 列表。 | `dataSource` |
| **`list_tables`** | 获取指定数据源下的表和视图列表，支持正则或通配符过滤。 | `dataSource` |
| **`describe_table`** | 查看指定表的字段结构定义（字段名、类型、主键、为空性等）。| `dataSource`, `table` |
| **`list_indexes`** | 获取指定表的索引信息（包含字段、唯一性等）。 | `dataSource`, `table` |
| **`get_foreign_keys`** | 获取指定表的外键关联关系。 | `dataSource`, `table` |
| **`get_table_ddl`** | 获取指定表的建表语句（DDL，不支持的数据库将降级拼装）。| `dataSource`, `table` |
| **`execute_query`** | 在指定数据源上安全地执行**只读 SQL 查询**。 | `dataSource`, `sql` |
| **`explain_query`** | 对目标数据源上的 SQL 查询进行执行计划分析（EXPLAIN）。 | `dataSource`, `sql` |
| **`count_rows`** | 快速统计表数据行数，支持安全的 WHERE 条件。 | `dataSource`, `table` |
| **`sample_data`** | 快速获取指定表的样例数据（默认最多返回 10 行）。 | `dataSource`, `table` |

---

## 📜 开源协议

本项目基于 MIT 协议开源。
