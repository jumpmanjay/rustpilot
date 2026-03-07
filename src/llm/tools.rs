//! Native tools that agents can use.
//!
//! Each tool has a name, description, input schema, and an execute function.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use wait_timeout::ChildExt;

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
        reg.register(MultiEditTool);
        reg.register(ListDirTool);
        reg.register(BashTool);
        reg.register(GrepTool);
        reg.register(GlobTool);
        reg.register(WebFetchTool);
        reg.register(TodoTool::new());
        reg.register(SkillTool::new());
        reg
    }
}

// ─── Read File ───

struct ReadFileTool;

impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "Read the contents of a file. Returns the file content with line numbers. For large files, use offset/limit to read specific sections.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to read (relative to project root)" },
                    "offset": { "type": "integer", "description": "Start line (1-indexed, default: 1)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read (default: all)" }
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
                let lines: Vec<&str> = content.lines().collect();
                let total = lines.len();
                let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
                let start = offset.saturating_sub(1);
                let limit = input["limit"].as_u64().map(|n| n as usize).unwrap_or(total);
                let end = (start + limit).min(total);

                let numbered: Vec<String> = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{:>4} | {}", start + i + 1, l))
                    .collect();

                let mut output = numbered.join("\n");
                if end < total {
                    output.push_str(&format!("\n\n({} more lines, {} total)", total - end, total));
                }

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
            description: "Edit a file by replacing exact text. The old_text must match exactly (including whitespace and indentation). Only the first match is replaced.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" },
                    "old_text": { "type": "string", "description": "Exact text to find (must match including whitespace)" },
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

// ─── Bash (shell command execution with timeout) ───

struct BashTool;

impl Tool for BashTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "bash".into(),
            description: "Run a shell command and return stdout/stderr. Use for compilation, tests, git, package management, etc. Commands run in bash.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "cwd": { "type": "string", "description": "Working directory (optional, defaults to project root)" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds (default: 120, max: 600)" }
                },
                "required": ["command"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let command = input["command"].as_str().unwrap_or("");
        let work_dir = input["cwd"]
            .as_str()
            .map(|p| resolve_path(cwd, p))
            .unwrap_or_else(|| cwd.to_string());
        let timeout_secs = input["timeout"].as_u64().unwrap_or(120).min(600);

        use std::process::Command;
        use std::time::Duration;

        let mut child = match Command::new("bash")
            .args(["-c", command])
            .current_dir(&work_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    tool_use_id: String::new(),
                    content: format!("Error spawning command: {}", e),
                    is_error: true,
                };
            }
        };

        // Wait with timeout
        let result = match child.wait_timeout(Duration::from_secs(timeout_secs)) {
            Ok(Some(status)) => {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        use std::io::Read;
                        let _ = s.read_to_string(&mut buf);
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        use std::io::Read;
                        let _ = s.read_to_string(&mut buf);
                        buf
                    })
                    .unwrap_or_default();

                let mut output = String::new();
                if !stdout.is_empty() {
                    output.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str("[stderr]\n");
                    output.push_str(&stderr);
                }

                // Truncate very long output
                if output.len() > 100_000 {
                    output.truncate(100_000);
                    output.push_str("\n... (truncated)");
                }

                let is_error = !status.success();
                if output.is_empty() {
                    output = format!(
                        "Command completed with exit code {}",
                        status.code().unwrap_or(-1)
                    );
                }
                (output, is_error)
            }
            Ok(None) => {
                // Timeout — kill the process
                let _ = child.kill();
                let _ = child.wait();
                (
                    format!("Command timed out after {}s and was killed", timeout_secs),
                    true,
                )
            }
            Err(e) => (format!("Error waiting for command: {}", e), true),
        };

        ToolResult {
            tool_use_id: String::new(),
            content: result.0,
            is_error: result.1,
        }
    }
}

// ─── Grep (native ripgrep engine) ───

struct GrepTool;

impl Tool for GrepTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "grep".into(),
            description: "Search for a regex pattern in files. Uses ripgrep's engine natively — fast, respects .gitignore, skips binary files. Supports full regex syntax.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Search pattern (regex by default, or literal with fixed_strings)" },
                    "path": { "type": "string", "description": "Directory to search in (default: project root)" },
                    "include": { "type": "string", "description": "File glob pattern to include (e.g., '*.rs', '*.py')" },
                    "fixed_strings": { "type": "boolean", "description": "Treat pattern as literal string, not regex (default: false)" },
                    "case_insensitive": { "type": "boolean", "description": "Case-insensitive search (default: true)" },
                    "max_results": { "type": "integer", "description": "Maximum number of matching lines (default: 200)" }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let pattern = input["pattern"].as_str().unwrap_or("");
        let search_dir = input["path"]
            .as_str()
            .map(|p| resolve_path(cwd, p))
            .unwrap_or_else(|| cwd.to_string());
        let include = input["include"].as_str();
        let fixed = input["fixed_strings"].as_bool().unwrap_or(false);
        let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(true);
        let max_results = input["max_results"].as_u64().unwrap_or(200) as usize;

        // Build the regex pattern
        let regex_pattern = if fixed {
            regex::escape(pattern)
        } else {
            pattern.to_string()
        };

        let matcher = match grep_regex::RegexMatcherBuilder::new()
            .case_insensitive(case_insensitive)
            .build(&regex_pattern)
        {
            Ok(m) => m,
            Err(e) => {
                return ToolResult {
                    tool_use_id: String::new(),
                    content: format!("Invalid pattern '{}': {}", pattern, e),
                    is_error: true,
                };
            }
        };

        let mut searcher = grep_searcher::SearcherBuilder::new()
            .binary_detection(grep_searcher::BinaryDetection::quit(b'\x00'))
            .line_number(true)
            .build();

        let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let results_clone = results.clone();
        let max = max_results;

        // Build walker with optional glob filter
        let mut walker_builder = ignore::WalkBuilder::new(&search_dir);
        walker_builder.hidden(true).max_depth(Some(20));

        // Apply include glob via types
        if let Some(glob) = include {
            let mut overrides = ignore::overrides::OverrideBuilder::new(&search_dir);
            let _ = overrides.add(glob);
            if let Ok(ov) = overrides.build() {
                walker_builder.overrides(ov);
            }
        }

        let walker = walker_builder.build();
        let search_dir_prefix = format!("{}/", search_dir);

        for entry in walker.flatten() {
            if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                continue;
            }

            {
                let r = results_clone.lock().unwrap();
                if r.len() >= max {
                    break;
                }
            }

            let file_path = entry.path();
            let relative = file_path
                .to_string_lossy()
                .strip_prefix(&search_dir_prefix)
                .unwrap_or(&file_path.to_string_lossy())
                .to_string();

            let results_inner = results_clone.clone();
            let rel = relative.clone();

            let _ = searcher.search_path(
                &matcher,
                file_path,
                grep_searcher::sinks::UTF8(move |line_num, line| {
                    let mut r = results_inner.lock().unwrap();
                    if r.len() >= max {
                        return Ok(false); // stop searching
                    }
                    r.push(format!("{}:{}: {}", rel, line_num, line.trim_end()));
                    Ok(true)
                }),
            );
        }

        let results = results.lock().unwrap();
        let truncated = results.len() >= max_results;

        let mut output = if results.is_empty() {
            format!("No matches for '{}'", pattern)
        } else {
            results.join("\n")
        };

        if truncated {
            output.push_str(&format!("\n\n(truncated at {} results)", max_results));
        }

        ToolResult {
            tool_use_id: String::new(),
            content: output,
            is_error: false,
        }
    }
}

// ─── Glob (find files by pattern — native, no external deps) ───

struct GlobTool;

impl Tool for GlobTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "glob".into(),
            description: "Find files by name pattern. Fast, native, respects .gitignore. Supports substring matching and glob patterns.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "File name pattern (substring or glob like '*.rs', 'test_*.py')" },
                    "path": { "type": "string", "description": "Directory to search in (default: project root)" },
                    "max_results": { "type": "integer", "description": "Maximum results (default: 100)" }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let pattern = input["pattern"].as_str().unwrap_or("");
        let search_dir = input["path"]
            .as_str()
            .map(|p| resolve_path(cwd, p))
            .unwrap_or_else(|| cwd.to_string());
        let max = input["max_results"].as_u64().unwrap_or(100) as usize;

        let pattern_lower = pattern.to_lowercase();
        let is_glob = pattern.contains('*') || pattern.contains('?');

        let mut walker_builder = ignore::WalkBuilder::new(&search_dir);
        walker_builder.hidden(true).max_depth(Some(20));

        // If it's a glob pattern, use overrides for filtering
        if is_glob {
            let mut overrides = ignore::overrides::OverrideBuilder::new(&search_dir);
            let _ = overrides.add(pattern);
            if let Ok(ov) = overrides.build() {
                walker_builder.overrides(ov);
            }
        }

        let walker = walker_builder.build();
        let search_dir_prefix = format!("{}/", search_dir);
        let mut results = Vec::new();

        for entry in walker.flatten() {
            if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                continue;
            }
            let path = entry.path().to_string_lossy();
            let relative = path
                .strip_prefix(&search_dir_prefix)
                .unwrap_or(&path)
                .to_string();

            // For non-glob patterns, do substring match
            if !is_glob && !relative.to_lowercase().contains(&pattern_lower) {
                continue;
            }

            results.push(relative);
            if results.len() >= max {
                break;
            }
        }

        let truncated = results.len() >= max;
        let mut output = if results.is_empty() {
            format!("No files matching '{}'", pattern)
        } else {
            results.join("\n")
        };

        if truncated {
            output.push_str(&format!("\n\n(truncated at {} results)", max));
        }

        ToolResult {
            tool_use_id: String::new(),
            content: output,
            is_error: false,
        }
    }
}

// ─── Multi Edit (multiple edits in one file, applied in order) ───

struct MultiEditTool;

impl Tool for MultiEditTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "multi_edit".into(),
            description: "Apply multiple find-and-replace edits to a single file in one call. Each edit is applied sequentially. More efficient than multiple edit_file calls.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" },
                    "edits": {
                        "type": "array",
                        "description": "Array of edits to apply in order",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_text": { "type": "string", "description": "Exact text to find" },
                                "new_text": { "type": "string", "description": "Replacement text" }
                            },
                            "required": ["old_text", "new_text"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        }
    }

    fn execute(&self, input: &Value, cwd: &str) -> ToolResult {
        let path = input["path"].as_str().unwrap_or("");
        let full_path = resolve_path(cwd, path);
        let edits = input["edits"].as_array();

        let Some(edits) = edits else {
            return ToolResult {
                tool_use_id: String::new(),
                content: "Error: 'edits' must be an array".into(),
                is_error: true,
            };
        };

        let mut content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    tool_use_id: String::new(),
                    content: format!("Error reading {}: {}", path, e),
                    is_error: true,
                };
            }
        };

        let mut applied = 0;
        let mut errors = Vec::new();

        for (i, edit) in edits.iter().enumerate() {
            let old_text = edit["old_text"].as_str().unwrap_or("");
            let new_text = edit["new_text"].as_str().unwrap_or("");
            if content.contains(old_text) {
                content = content.replacen(old_text, new_text, 1);
                applied += 1;
            } else {
                errors.push(format!("Edit {}: old_text not found", i + 1));
            }
        }

        if applied > 0 {
            if let Err(e) = std::fs::write(&full_path, &content) {
                return ToolResult {
                    tool_use_id: String::new(),
                    content: format!("Error writing {}: {}", path, e),
                    is_error: true,
                };
            }
        }

        let mut msg = format!("Applied {}/{} edits to {}", applied, edits.len(), path);
        if !errors.is_empty() {
            msg.push_str(&format!("\nErrors:\n{}", errors.join("\n")));
        }

        ToolResult {
            tool_use_id: String::new(),
            content: msg,
            is_error: !errors.is_empty(),
        }
    }
}

// ─── Web Fetch (retrieve URL content) ───

struct WebFetchTool;

impl Tool for WebFetchTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "web_fetch".into(),
            description: "Fetch content from a URL. Returns the response body as text. Useful for reading documentation, APIs, or web pages. Uses curl under the hood.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch (http or https)" },
                    "max_bytes": { "type": "integer", "description": "Maximum response size in bytes (default: 100000)" },
                    "headers": {
                        "type": "object",
                        "description": "Optional HTTP headers as key-value pairs",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["url"]
            }),
        }
    }

    fn execute(&self, input: &Value, _cwd: &str) -> ToolResult {
        let url = input["url"].as_str().unwrap_or("");
        let max_bytes = input["max_bytes"].as_u64().unwrap_or(100_000);

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ToolResult {
                tool_use_id: String::new(),
                content: "Error: URL must start with http:// or https://".into(),
                is_error: true,
            };
        }

        let mut cmd = std::process::Command::new("curl");
        cmd.args([
            "-sS",
            "-L",                    // follow redirects
            "--max-time", "30",      // 30s timeout
            "--max-filesize", &max_bytes.to_string(),
            "-H", "User-Agent: RustPilot/0.1",
        ]);

        // Add custom headers
        if let Some(headers) = input["headers"].as_object() {
            for (key, val) in headers {
                if let Some(v) = val.as_str() {
                    cmd.args(["-H", &format!("{}: {}", key, v)]);
                }
            }
        }

        cmd.arg(url);

        match cmd.output() {
            Ok(output) => {
                let mut body = String::from_utf8_lossy(&output.stdout).to_string();
                if body.len() > max_bytes as usize {
                    body.truncate(max_bytes as usize);
                    body.push_str("\n... (truncated)");
                }
                let stderr = String::from_utf8_lossy(&output.stderr);

                if !output.status.success() {
                    return ToolResult {
                        tool_use_id: String::new(),
                        content: format!("Error fetching {}: {}", url, stderr),
                        is_error: true,
                    };
                }

                ToolResult {
                    tool_use_id: String::new(),
                    content: body,
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                tool_use_id: String::new(),
                content: format!("Error: curl not available or failed: {}", e),
                is_error: true,
            },
        }
    }
}

// ─── Todo (task tracking within a session) ───

struct TodoTool {
    todos: std::sync::Arc<std::sync::Mutex<Vec<TodoItem>>>,
}

#[derive(Debug, Clone)]
struct TodoItem {
    id: usize,
    text: String,
    status: TodoStatus,
}

#[derive(Debug, Clone, PartialEq)]
enum TodoStatus {
    Pending,
    InProgress,
    Done,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "⬚"),
            TodoStatus::InProgress => write!(f, "◑"),
            TodoStatus::Done => write!(f, "✓"),
        }
    }
}

impl TodoTool {
    fn new() -> Self {
        Self {
            todos: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

impl Tool for TodoTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "todo".into(),
            description: "Track tasks and progress. Actions: 'add' (create task), 'update' (change status), 'list' (show all tasks). Use this to plan multi-step work and track what's done.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "update", "list"],
                        "description": "Action to perform"
                    },
                    "text": { "type": "string", "description": "Task description (for 'add')" },
                    "id": { "type": "integer", "description": "Task ID (for 'update')" },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "done"],
                        "description": "New status (for 'update')"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn execute(&self, input: &Value, _cwd: &str) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("list");
        let mut todos = self.todos.lock().unwrap();

        match action {
            "add" => {
                let text = input["text"].as_str().unwrap_or("(untitled)");
                let id = todos.len() + 1;
                todos.push(TodoItem {
                    id,
                    text: text.to_string(),
                    status: TodoStatus::Pending,
                });
                ToolResult {
                    tool_use_id: String::new(),
                    content: format!("Added task #{}: {}", id, text),
                    is_error: false,
                }
            }
            "update" => {
                let id = input["id"].as_u64().unwrap_or(0) as usize;
                let status_str = input["status"].as_str().unwrap_or("done");
                let status = match status_str {
                    "pending" => TodoStatus::Pending,
                    "in_progress" => TodoStatus::InProgress,
                    "done" => TodoStatus::Done,
                    _ => TodoStatus::Pending,
                };

                if let Some(item) = todos.iter_mut().find(|t| t.id == id) {
                    item.status = status;
                    ToolResult {
                        tool_use_id: String::new(),
                        content: format!("Updated task #{} → {}", id, item.status),
                        is_error: false,
                    }
                } else {
                    ToolResult {
                        tool_use_id: String::new(),
                        content: format!("Task #{} not found", id),
                        is_error: true,
                    }
                }
            }
            "list" | _ => {
                if todos.is_empty() {
                    return ToolResult {
                        tool_use_id: String::new(),
                        content: "No tasks.".into(),
                        is_error: false,
                    };
                }
                let list: Vec<String> = todos
                    .iter()
                    .map(|t| format!("  {} #{}: {}", t.status, t.id, t.text))
                    .collect();

                let done = todos.iter().filter(|t| t.status == TodoStatus::Done).count();
                let total = todos.len();
                let mut output = list.join("\n");
                output.push_str(&format!("\n\nProgress: {}/{}", done, total));

                ToolResult {
                    tool_use_id: String::new(),
                    content: output,
                    is_error: false,
                }
            }
        }
    }
}

// ─── Skill (load on-demand skill instructions) ───

struct SkillTool {
    /// Map of skill name → (description, SKILL.md path)
    skills: HashMap<String, (String, String)>,
}

impl SkillTool {
    fn new() -> Self {
        let config = crate::config::Config::load_or_default().unwrap_or_else(|_| {
            panic!("Failed to load config for skill discovery");
        });

        let skills: HashMap<String, (String, String)> = config
            .skills
            .iter()
            .map(|(name, def)| {
                (
                    name.clone(),
                    (
                        def.description.clone(),
                        def.path.to_string_lossy().to_string(),
                    ),
                )
            })
            .collect();

        Self { skills }
    }
}

impl Tool for SkillTool {
    fn definition(&self) -> ToolDef {
        let mut desc = "Load a skill's full instructions by name. Available skills:".to_string();
        if self.skills.is_empty() {
            desc.push_str("\n(no skills discovered)");
        } else {
            for (name, (description, _)) in &self.skills {
                desc.push_str(&format!("\n- {}: {}", name, description));
            }
        }

        ToolDef {
            name: "skill".into(),
            description: desc,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name of the skill to load" }
                },
                "required": ["name"]
            }),
        }
    }

    fn execute(&self, input: &Value, _cwd: &str) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");

        if let Some((_, path)) = self.skills.get(name) {
            match std::fs::read_to_string(path) {
                Ok(content) => ToolResult {
                    tool_use_id: String::new(),
                    content,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    tool_use_id: String::new(),
                    content: format!("Error loading skill '{}': {}", name, e),
                    is_error: true,
                },
            }
        } else {
            let available: Vec<&str> = self.skills.keys().map(|s| s.as_str()).collect();
            ToolResult {
                tool_use_id: String::new(),
                content: format!(
                    "Unknown skill '{}'. Available: {}",
                    name,
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                ),
                is_error: true,
            }
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
