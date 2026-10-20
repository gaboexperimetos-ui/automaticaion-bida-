//! MCP tool definitions and handlers.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Tool descriptor for MCP tools/list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    /// JSON Schema de los argumentos de la tool.
    ///
    /// Se serializa como `inputSchema`, que es el nombre de campo que exige la
    /// spec MCP (`2025-03-26`): el cliente lee el schema de
    /// `tool.inputSchema` y descarta cualquier otra clave. Emitirlo como
    /// `parameters` dejaba las 9 tools sin schema utilizable — el cliente
    /// recibía `{"type":"object","properties":{…}}` en una clave que nadie
    /// lee, y por tanto no sabía qué argumentos pasar.
    ///
    /// `alias` mantiene la deserialización tolerante con el nombre antiguo.
    #[serde(rename = "inputSchema", alias = "parameters")]
    pub input_schema: Value,
}

/// List of all available tools
pub fn all_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "search_notes".to_string(),
            description: "Search notes by title or content".to_string(),
            input_schema: json!({
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
            input_schema: json!({
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
            input_schema: json!({
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
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        McpTool {
            name: "get_subject_notes".to_string(),
            description: "Get notes of a subject (matched by tag, with full-text fallback)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "Subject name, matched against note tags"}
                },
                "required": ["subject"]
            }),
        },
        McpTool {
            name: "get_study_sheets".to_string(),
            description: "Get study material (notes with their body) to build study sheets from".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "Optional subject to narrow the material"}
                },
                "required": []
            }),
        },
        McpTool {
            name: "app_list_backups".to_string(),
            description: "List database backups in data/backups, newest first".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        McpTool {
            name: "app_backup_now".to_string(),
            description: "Create a backup of the SQLite database now".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        McpTool {
            name: "app_ai_cost".to_string(),
            description: "Current month AI spend, budget limit and request count".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tools_list_has_all_nine() {
        let tools = all_tools();
        assert_eq!(tools.len(), 9);
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_notes"));
        assert!(names.contains(&"get_note"));
        assert!(names.contains(&"create_note"));
        assert!(names.contains(&"get_academic_context"));
        assert!(names.contains(&"get_subject_notes"));
        assert!(names.contains(&"get_study_sheets"));
        assert!(names.contains(&"app_list_backups"));
        assert!(names.contains(&"app_backup_now"));
        assert!(names.contains(&"app_ai_cost"));
    }

    #[test]
    fn test_tool_parameters() {
        let tools = all_tools();
        let search = tools.iter().find(|t| t.name == "search_notes").unwrap();
        assert!(search.input_schema["properties"].get("query").is_some());
        let required = search.input_schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "query");
    }

    /// Contenido exacto de la lista: 9 tools, con estos nombres y en este
    /// orden, cada una con descripción y esquema de parámetros utilizable.
    #[test]
    fn test_tools_list_exact_contents() {
        let tools = all_tools();
        assert_eq!(tools.len(), 9);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "search_notes",
                "get_note",
                "create_note",
                "get_academic_context",
                "get_subject_notes",
                "get_study_sheets",
                "app_list_backups",
                "app_backup_now",
                "app_ai_cost",
            ]
        );

        for t in &tools {
            assert!(
                !t.description.trim().is_empty(),
                "la tool {} no tiene descripción",
                t.name
            );
            assert_eq!(
                t.input_schema["type"],
                json!("object"),
                "la tool {} no declara un schema object",
                t.name
            );
            assert!(
                t.input_schema.get("required").is_some(),
                "la tool {} no declara `required`",
                t.name
            );
        }
    }

    /// Las tres tools de operación exponen un schema de parámetros sin
    /// argumentos: se llaman sin `arguments`.
    #[test]
    fn test_app_tools_take_no_arguments() {
        for name in ["app_list_backups", "app_backup_now", "app_ai_cost"] {
            let t = all_tools().into_iter().find(|t| t.name == name).unwrap();
            assert_eq!(t.input_schema["properties"], json!({}));
            assert_eq!(t.input_schema["required"], json!([]));
        }
    }

    /// La spec MCP exige que el schema de argumentos se exponga en
    /// `inputSchema`. Si la clave se llama de otro modo, un cliente conforme
    /// recibe la tool sin schema y no sabe qué argumentos enviar.
    #[test]
    fn test_serialized_tool_exposes_input_schema_key() {
        let tools = all_tools();
        for t in &tools {
            let value = serde_json::to_value(t).expect("la tool debe serializarse");
            assert!(
                value.get("inputSchema").is_some(),
                "la tool {} no expone `inputSchema`",
                t.name
            );
            assert!(
                value.get("parameters").is_none(),
                "la tool {} sigue exponiendo la clave antigua `parameters`",
                t.name
            );
            assert_eq!(value["inputSchema"]["type"], json!("object"));
        }
    }

    /// Cada tool con argumentos declara sus `required` de forma coherente con
    /// las propiedades: ningún `required` apunta a una propiedad inexistente.
    #[test]
    fn test_required_properties_always_exist() {
        for t in all_tools() {
            let properties = t.input_schema["properties"].as_object();
            let required = t.input_schema["required"].as_array();
            if let (Some(props), Some(req)) = (properties, required) {
                for key in req {
                    let key = key.as_str().unwrap();
                    assert!(
                        props.contains_key(key),
                        "la tool {} marca `{}` como required pero no lo declara en properties",
                        t.name,
                        key
                    );
                }
            }
        }
    }
}
