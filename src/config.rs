use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use regex::Regex;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub mcp: McpConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct McpConfig {
    #[serde(rename = "dataSources")]
    pub data_sources: HashMap<String, DataSourceConfig>,
    pub security: SecurityConfig,
    pub audit: AuditConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DataSourceConfig {
    #[serde(rename = "type")]
    pub db_type: String, // MYSQL, POSTGRESQL, SQLITE, SQLSERVER
    #[serde(rename = "jdbcUrl")]
    pub jdbc_url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub dsn: Option<String>, // 允许直接配置 Rust DSN
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecurityConfig {
    #[serde(rename = "allowedPrefixes")]
    pub allowed_prefixes: Vec<String>,
    #[serde(rename = "maxRows")]
    pub max_rows: usize,
    #[serde(rename = "queryTimeout")]
    pub query_timeout: u64,
    #[serde(rename = "maxResultSetSizeBytes")]
    pub max_result_set_size_bytes: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuditConfig {
    pub enabled: bool,
    #[serde(rename = "logFile")]
    pub log_file: String,
    #[serde(rename = "logResults")]
    pub log_results: Option<bool>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allowed_prefixes: vec![
                "select".to_string(),
                "show".to_string(),
                "desc".to_string(),
                "explain".to_string(),
            ],
            max_rows: 1000,
            query_timeout: 30,
            max_result_set_size_bytes: 10 * 1024 * 1024, // 10MB
        }
    }
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            log_file: "./logs/query-audit.log".to_string(),
            log_results: Some(false),
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            data_sources: HashMap::new(),
            security: SecurityConfig::default(),
            audit: AuditConfig::default(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mcp: McpConfig::default(),
        }
    }
}

pub fn load_config<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = serde_yaml::from_str(&content)?;
    Ok(config)
}

// 将 JDBC URL 或者是 host/port/database 配置转为 sqlx 支持的连接串
pub fn convert_to_dsn(cfg: &DataSourceConfig) -> Result<String, anyhow::Error> {
    let db_type = cfg.db_type.to_uppercase();

    // 如果直接配置了 dsn
    if let Some(ref dsn) = cfg.dsn {
        return Ok(dsn.clone());
    }

    // 如果配置了 jdbcUrl
    if let Some(ref jdbc_url) = cfg.jdbc_url {
        return parse_jdbc_url(jdbc_url, cfg.username.as_deref().unwrap_or(""), cfg.password.as_deref().unwrap_or(""));
    }

    let username = cfg.username.as_deref().unwrap_or("");
    let password = cfg.password.as_deref().unwrap_or("");
    let database = cfg.database.as_deref().unwrap_or("");
    let host = cfg.host.as_deref().unwrap_or("localhost");

    match db_type.as_str() {
        "MYSQL" | "MARIADB" | "GBASE" => {
            let port = cfg.port.unwrap_or(3306);
            let mut dsn = format!("mysql://{}:{}@{}:{}/{}", username, password, host, port, database);
            if let Some(ref props) = cfg.properties {
                if !props.is_empty() {
                    let parts: Vec<String> = props.iter().map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))).collect();
                    dsn = format!("{}?{}", dsn, parts.join("&"));
                }
            }
            Ok(dsn)
        }
        "POSTGRESQL" | "KINGBASE" | "HIGHGO" => {
            let port = cfg.port.unwrap_or(5432);
            let sslmode = if let Some(ref props) = cfg.properties {
                if let Some(val) = props.get("sslmode") {
                    val.clone()
                } else if let Some(val) = props.get("ssl") {
                    if val == "true" { "require".to_string() } else { "disable".to_string() }
                } else {
                    "disable".to_string()
                }
            } else {
                "disable".to_string()
            };
            let mut dsn = format!("postgres://{}:{}@{}:{}/{}?sslmode={}", 
                urlencoding::encode(username), urlencoding::encode(password), host, port, database, sslmode);
            if let Some(ref props) = cfg.properties {
                for (k, v) in props {
                    if k != "sslmode" && k != "ssl" {
                        dsn = format!("{}&{}={}", dsn, urlencoding::encode(k), urlencoding::encode(v));
                    }
                }
            }
            Ok(dsn)
        }
        "SQLITE" => {
            let db_path = if database.is_empty() { ":memory:" } else { database };
            Ok(format!("sqlite://{}", db_path))
        }
        "SQLSERVER" => {
            let port = cfg.port.unwrap_or(1433);
            let mut dsn = format!("mssql://{}:{}@{}:{}/{}", username, password, host, port, database);
            if let Some(ref props) = cfg.properties {
                if !props.is_empty() {
                    let parts: Vec<String> = props.iter().map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))).collect();
                    dsn = format!("{}?{}", dsn, parts.join("&"));
                }
            }
            Ok(dsn)
        }
        _ => Err(anyhow::anyhow!("Unsupported database type: {}", cfg.db_type)),
    }
}

fn parse_jdbc_url(jdbc_url: &str, username: &str, password: &str) -> Result<String, anyhow::Error> {
    if !jdbc_url.starts_with("jdbc:") {
        return Err(anyhow::anyhow!("Invalid JDBC URL: must start with 'jdbc:'"));
    }
    let sub = &jdbc_url[5..];
    let idx = sub.find(':').ok_or_else(|| anyhow::anyhow!("Invalid JDBC URL: missing protocol"))?;
    let proto = &sub[..idx].to_lowercase();
    let rem = &sub[idx+1..];

    match proto.as_str() {
        "mysql" | "mariadb" => {
            let rem = rem.trim_start_matches("//");
            let parts: Vec<&str> = rem.splitn(2, '?').collect();
            let addr_and_db = parts[0];
            let params = if parts.len() > 1 { format!("?{}", parts[1]) } else { "".to_string() };
            
            let addr_parts: Vec<&str> = addr_and_db.splitn(2, '/').collect();
            let host_port = addr_parts[0];
            let db_name = if addr_parts.len() > 1 { addr_parts[1] } else { "" };
            
            let host_port = if !host_port.contains(':') {
                format!("{}:3306", host_port)
            } else {
                host_port.to_string()
            };

            Ok(format!("mysql://{}:{}@{}/{}{}", username, password, host_port, db_name, params))
        }
        "postgresql" | "kingbase8" | "highgo" => {
            let rem = rem.trim_start_matches("//");
            let parts: Vec<&str> = rem.splitn(2, '?').collect();
            let addr_and_db = parts[0];
            let params = if parts.len() > 1 { format!("?{}", parts[1]) } else { "".to_string() };

            let addr_parts: Vec<&str> = addr_and_db.splitn(2, '/').collect();
            let host_port = addr_parts[0];
            let db_name = if addr_parts.len() > 1 { addr_parts[1] } else { "" };

            let mut host = host_port.to_string();
            let mut port = "5432".to_string();
            if host_port.contains(':') {
                let hp: Vec<&str> = host_port.splitn(2, ':').collect();
                host = hp[0].to_string();
                port = hp[1].to_string();
            }

            let mut sslmode = "disable".to_string();
            if params.contains("ssl=true") {
                sslmode = "require".to_string();
            }

            let mut dsn = format!("postgres://{}:{}@{}:{}/{}{}", 
                urlencoding::encode(username), urlencoding::encode(password), host, port, db_name, params);
            if !dsn.contains("sslmode=") {
                if dsn.contains('?') {
                    dsn = format!("{}&sslmode={}", dsn, sslmode);
                } else {
                    dsn = format!("{}?sslmode={}", dsn, sslmode);
                }
            }
            Ok(dsn)
        }
        "sqlite" => {
            let db_path = rem.trim_start_matches(':');
            Ok(format!("sqlite://{}", db_path))
        }
        "sqlserver" => {
            let rem = rem.trim_start_matches("//");
            let parts: Vec<&str> = rem.split(';').collect();
            let host_port = parts[0];
            
            let mut host = host_port.to_string();
            let mut port = "1433".to_string();
            if host_port.contains(':') {
                let hp: Vec<&str> = host_port.splitn(2, ':').collect();
                host = hp[0].to_string();
                port = hp[1].to_string();
            }

            let mut db_name = "";
            let mut extra = Vec::new();
            for part in parts.iter().skip(1) {
                if part.is_empty() { continue; }
                let kv: Vec<&str> = part.splitn(2, '=').collect();
                if kv.len() == 2 {
                    let k = kv[0].trim().to_lowercase();
                    let v = kv[1].trim();
                    if k == "databasename" || k == "database" {
                        db_name = v;
                    } else {
                        extra.push(format!("{}={}", urlencoding::encode(&k), urlencoding::encode(v)));
                    }
                }
            }

            let mut dsn = format!("mssql://{}:{}@{}:{}/{}", 
                urlencoding::encode(username), urlencoding::encode(password), host, port, db_name);
            if !extra.is_empty() {
                dsn = format!("{}?{}", dsn, extra.join("&"));
            }
            Ok(dsn)
        }
        _ => {
            if proto.contains("kingbase") {
                parse_jdbc_url(&format!("jdbc:postgresql:{}", rem), username, password)
            } else {
                Err(anyhow::anyhow!("Unsupported jdbcUrl protocol: {}", proto))
            }
        }
    }
}

pub fn normalize_sql(sql: &str) -> String {
    // 匹配单/双引号、反引号字面量，多行注释，单行注释
    let re = Regex::new(r#"('(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*"|`[^`]*`)|(/\*(?s).*?\*/)|(?m)(?:--|#).*?(?:\r?\n|$)"#).unwrap();
    let result = re.replace_all(sql, |caps: &regex::Captures| {
        if let Some(m) = caps.get(1) {
            m.as_str().to_string()
        } else {
            " ".to_string()
        }
    });
    
    // 压缩空白
    let space_re = Regex::new(r#"\s+"#).unwrap();
    space_re.replace_all(&result, " ").trim().to_string()
}

pub fn contains_subquery(where_clause: &str) -> bool {
    if where_clause.trim().is_empty() {
        return false;
    }
    // 过滤字符串和注释
    let re = Regex::new(r#"('(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*"|`[^`]*`)|(/\*(?s).*?\*/)|(?m)(?:--|#).*?(?:\r?\n|$)"#).unwrap();
    let clean = re.replace_all(where_clause, |caps: &regex::Captures| {
        if caps.get(1).is_some() {
            "''".to_string()
        } else {
            " ".to_string()
        }
    });
    let clean_lower = clean.to_lowercase();
    let select_re = Regex::new(r#"\bselect\b"#).unwrap();
    select_re.is_match(&clean_lower)
}

pub fn wildcard_to_regex(pattern: &str) -> String {
    if pattern.is_empty() {
        return "".to_string();
    }
    let mut sb = String::new();
    sb.push('^');
    for c in pattern.chars() {
        match c {
            '%' => sb.push_str(".*"),
            '_' => sb.push('.'),
            '\\' | '.' | '*' | '+' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' => {
                sb.push('\\');
                sb.push(c);
            }
            _ => sb.push(c),
        }
    }
    sb.push('$');
    sb
}
