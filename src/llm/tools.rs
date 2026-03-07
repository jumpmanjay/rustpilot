//! Native tools that agents can use.
//!
//! Each tool has a name, description, input schema, and an execute function.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// Tool definition sent to the LLM API
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Result of executing a tool
#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Registry of available tools
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool + Send + Sync>>,
}

pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDef;
    fn execute(&self, input: &Value, cwd: &str) -> ToolResult;
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        let def = tool.definition();
        self.tools.insert(def.name.clone(), Box::new(tool));
    }

    pub fn definitions(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    pub fn execute(&self, name: &str, tool_use_id: &str, input: &Value, cwd: &str) -> ToolResult {
        if let Some(tool) = self.tools.get(name) {
            let mut result = tool.execute(input, cwd);
            result.tool_use_id = tool_use_id.to_string();
            result
        } else {
            ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: format!("Unknown tool: {}", name),
                is_error: true,
            }
        }
    }

    /// Build the default set of native tools
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        reg.register(ReadFileTool);
        reg.register(WriteFileTool);
        reg.register(EditFileTool);
        reg.register(ListDirTool);
        reg.register(RunCommandTool);
        reg.register(SearchFilesTool);
        reg.register(GrepTool);
        reg
    }
}

// ─── Read File ───

struct ReadFileTool;

impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "Read the contents of a file. Returns the file content as text.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to read" },
                    "start_line": { "type": "integer", "description": "Optional start line (1-indexed)" },
                    "end_line": { "type": "integer", "description": "Optional end line (inclusive)" }
                },
                "required": ["path"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let path = input["path"].as_str().unwrap_or("");
        let full_path = resolve_path(cwd, path);

        match std::fs::read_to_string(&full_path) {
            Ok(content) => {
                let start = input["start_line"].as_u64().map(|n| n as usize);
                let end = input["end_line"].as_u64().map(|n| n as usize);

                let output = if let Some(start) = start {
                    let lines: Vec<&str> = content.lines().collect();
                    let start_idx = start.saturating_sub(1);
                    let end_idx = end.unwrap_or(lines.len()).min(lines.len());
                    lines[start_idx..end_idx]
                        .iter()
                        .enumerate()
                        .map(|(i, l)| format!("{:>4} | {}", start_idx + i + 1, l))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    content
                };

                ToolResult {
                    tool_use_id: String::new(),
                    content: output,
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                tool_use_id: String::new(),
                content: format!("Error reading {}: {}", path, e),
                is_error: true,
            },
        }
    }
}

// ─── Write File ───

struct WriteFileTool;

impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "write_file".into(),
            description: "Write content to a file. Creates parent directories if needed. Overwrites existing content.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let path = input["path"].as_str().unwrap_or("");
        let content = input["content"].as_str().unwrap_or("");
        let full_path = resolve_path(cwd, path);

        // Create parent dirs
        if let Some(parent) = std::path::Path::new(&full_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match std::fs::write(&full_path, content) {
            Ok(_) => ToolResult {
                tool_use_id: String::new(),
                content: format!("Wrote {} bytes to {}", content.len(), path),
                is_error: false,
            },
            Err(e) => ToolResult {
                tool_use_id: String::new(),
                content: format!("Error writing {}: {}", path, e),
                is_error: true,
            },
        }
    }
}

// ─── Edit File (find & replace) ───

struct EditFileTool;

impl Tool for EditFileTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "edit_file".into(),
            description: "Edit a file by replacing exact text. The old_text must match exactly (including whitespace).".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" },
                    "old_text": { "type": "string", "description": "Exact text to find" },
                    "new_text": { "type": "string", "description": "Replacement text" }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let path = input["path"].as_str().unwrap_or("");
        let old_text = input["old_text"].as_str().unwrap_or("");
        let new_text = input["new_text"].as_str().unwrap_or("");
        let full_path = resolve_path(cwd, path);

        match std::fs::read_to_string(&full_path) {
            Ok(content) => {
                if !content.contains(old_text) {
                    return ToolResult {
                        tool_use_id: String::new(),
                        content: format!("Error: old_text not found in {}", path),
                        is_error: true,
                    };
                }
                let new_content = content.replacen(old_text, new_text, 1);
                match std::fs::write(&full_path, &new_content) {
                    Ok(_) => ToolResult {
                        tool_use_id: String::new(),
                        content: format!("Successfully edited {}", path),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_use_id: String::new(),
                        content: format!("Error writing {}: {}", path, e),
                        is_error: true,
                    },
                }
            }
            Err(e) => ToolResult {
                tool_use_id: String::new(),
                content: format!("Error reading {}: {}", path, e),
                is_error: true,
            },
        }
    }
}

// ─── List Directory ───

struct ListDirTool;

impl Tool for ListDirTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "list_dir".into(),
            description: "List files and directories at a path. Shows file sizes and types.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path (default: current directory)" }
                },
                "required": []
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let path = input["path"].as_str().unwrap_or(".");
        let full_path = resolve_path(cwd, path);

        match std::fs::read_dir(&full_path) {
            Ok(entries) => {
                let mut items: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                        let name = e.file_name().to_string_lossy().to_string();
                        if is_dir {
                            format!("  📁 {}/", name)
                        } else {
                            format!("  📄 {} ({})", name, human_size(size))
                        }
                    })
                    .collect();
                items.sort();
                ToolResult {
                    tool_use_id: String::new(),
                    content: items.join("\n"),
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                tool_use_id: String::new(),
                content: format!("Error listing {}: {}", path, e),
                is_error: true,
            },
        }
    }
}

// ─── Run Command ───

struct RunCommandTool;

impl Tool for RunCommandTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "run_command".into(),
            description: "Run a shell command and return its output. Use for compilation, tests, git, etc.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "cwd": { "type": "string", "description": "Working directory (optional, defaults to project root)" }
                },
                "required": ["command"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let command = input["command"].as_str().unwrap_or("");
        let work_dir = input["cwd"].as_str().map(|p| resolve_path(cwd, p)).unwrap_or_else(|| cwd.to_string());

        match std::process::Command::new("sh")
            .args(["-c", command])
            .current_dir(&work_dir)
            .output()
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str("[stderr]\n");
                    result.push_str(&stderr);
                }
                // Truncate very long output
                if result.len() > 50000 {
                    result.truncate(50000);
                    result.push_str("\n... (truncated)");
                }
                ToolResult {
                    tool_use_id: String::new(),
                    content: if result.is_empty() {
                        format!("Command completed with exit code {}", output.status.code().unwrap_or(-1))
                    } else {
                        result
                    },
                    is_error: !output.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_use_id: String::new(),
                content: format!("Error running command: {}", e),
                is_error: true,
            },
        }
    }
}

// ─── Search Files (find by name) ───

struct SearchFilesTool;

impl Tool for SearchFilesTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "search_files".into(),
            description: "Search for files by name pattern in the workspace. Respects .gitignore.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "File name pattern to search for (case-insensitive substring)" },
                    "path": { "type": "string", "description": "Directory to search in (default: workspace root)" }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let pattern = input["pattern"].as_str().unwrap_or("");
        let search_dir = input["path"].as_str().map(|p| resolve_path(cwd, p)).unwrap_or_else(|| cwd.to_string());
        let pattern_lower = pattern.to_lowercase();

        let mut results = Vec::new();
        let walker = ignore::WalkBuilder::new(&search_dir)
            .hidden(true)
            .max_depth(Some(10))
            .build();

        for entry in walker.flatten() {
            if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                continue;
            }
            let path = entry.path().to_string_lossy();
            if path.to_lowercase().contains(&pattern_lower) {
                let relative = path
                    .strip_prefix(&search_dir)
                    .unwrap_or(&path)
                    .trim_start_matches('/');
                results.push(relative.to_string());
                if results.len() >= 50 {
                    break;
                }
            }
        }

        ToolResult {
            tool_use_id: String::new(),
            content: if results.is_empty() {
                format!("No files matching '{}'", pattern)
            } else {
                results.join("\n")
            },
            is_error: false,
        }
    }
}

// ─── Grep (search file contents) ───

struct GrepTool;

impl Tool for GrepTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "grep".into(),
            description: "Search for text in files across the workspace. Returns matching lines with file paths and line numbers.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Text to search for (case-insensitive)" },
                    "path": { "type": "string", "description": "Directory to search in (default: workspace root)" },
                    "file_pattern": { "type": "string", "description": "Optional file name filter (e.g., '*.rs')" }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let pattern = input["pattern"].as_str().unwrap_or("");
        let search_dir = input["path"].as_str().map(|p| resolve_path(cwd, p)).unwrap_or_else(|| cwd.to_string());
        let file_pattern = input["file_pattern"].as_str();
        let pattern_lower = pattern.to_lowercase();

        let mut results = Vec::new();
        let walker = ignore::WalkBuilder::new(&search_dir)
            .hidden(true)
            .max_depth(Some(10))
            .build();

        for entry in walker.flatten() {
            if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                continue;
            }
            let path = entry.path();
            let path_str = path.to_string_lossy();

            // File pattern filter
            if let Some(fp) = file_pattern {
                let fp_clean = fp.trim_start_matches('*');
                if !path_str.ends_with(fp_clean) {
                    continue;
                }
            }

            if let Ok(content) = std::fs::read_to_string(path) {
                let relative = path_str
                    .strip_prefix(&search_dir)
                    .unwrap_or(&path_str)
                    .trim_start_matches('/');
                for (i, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&pattern_lower) {
                        results.push(format!("{}:{}: {}", relative, i + 1, line.trim()));
                        if results.len() >= 100 {
                            break;
                        }
                    }
                }
            }
            if results.len() >= 100 {
                break;
            }
        }

        ToolResult {
            tool_use_id: String::new(),
            content: if results.is_empty() {
                format!("No matches for '{}'", pattern)
            } else {
                results.join("\n")
            },
            is_error: false,
        }
    }
}

// ─── Helpers ───

fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{}", cwd, path)
    }
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
