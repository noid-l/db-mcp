#[derive(Clone)]
pub struct SqlValidator {
    allowed_prefixes: Vec<String>,
}

impl SqlValidator {
    pub fn new(allowed_prefixes: Vec<String>) -> Self {
        Self { allowed_prefixes }
    }

    pub fn validate(&self, sql: &str) -> Result<(), anyhow::Error> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!("SQL query cannot be empty"));
        }

        let normalized = crate::config::normalize_sql(sql);
        let clean_lower = normalized.to_lowercase();

        if clean_lower.is_empty() {
            return Err(anyhow::anyhow!("SQL query contains only whitespace or comments"));
        }

        let mut has_allowed_prefix = false;
        for prefix in &self.allowed_prefixes {
            if clean_lower.starts_with(&prefix.trim().to_lowercase()) {
                has_allowed_prefix = true;
                break;
            }
        }

        if !has_allowed_prefix {
            return Err(anyhow::anyhow!(
                "security exception: only queries with allowed prefixes ({}) are permitted",
                self.allowed_prefixes.join(", ")
            ));
        }

        Ok(())
    }
}
