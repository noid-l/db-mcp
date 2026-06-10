use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: InputSchema,
}

#[derive(Debug, Serialize, Clone)]
pub struct InputSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: HashMap<String, Property>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Property {
    #[serde(rename = "type")]
    pub prop_type: String,
    pub description: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub description: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Prompt {
    pub name: String,
    pub description: String,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Clone)]
pub struct PromptMessage {
    pub role: String,
    pub content: PromptContent,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Clone)]
pub struct PromptContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}
