//! MCP tool definitions and handlers.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Tool descriptor for MCP tools/list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// List of all available tools
pub fn all_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "search_notes".to_string(),
            description: "Search notes by title or content".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "limit": {"type": "integer", "description": "Max results", "default": 10}
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "get_note".to_string(),
            description: "Get a note by slug".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "slug": {"type": "string", "description": "Note slug"}
                },
                "required": ["slug"]
            }),
        },
        McpTool {
            name: "create_note".to_string(),
            description: "Create a new Markdown note".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "content": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["title", "content"]
            }),
        },
        McpTool {
            name: "get_academic_context".to_string(),
            description: "Get calendar, schedule, and upcoming events".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        McpTool {
            name: "get_subject_notes".to_string(),
            description: "Get notes filtered by subject tag".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string"}
                },
                "required": ["subject"]
            }),
        },
        McpTool {
            name: "get_study_sheets".to_string(),
            description: "Get generated study sheets".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string"}
                },
                "required": []
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tools_list_has_all_six() {
        let tools = all_tools();
        assert_eq!(tools.len(), 6);
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_notes"));
        assert!(names.contains(&"get_note"));
        assert!(names.contains(&"create_note"));
        assert!(names.contains(&"get_academic_context"));
        assert!(names.contains(&"get_subject_notes"));
        assert!(names.contains(&"get_study_sheets"));
    }

    #[test]
    fn test_tool_parameters() {
        let tools = all_tools();
        let search = tools.iter().find(|t| t.name == "search_notes").unwrap();
        assert!(search.parameters["properties"].get("query").is_some());
        let required = search.parameters["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "query");
    }
}
