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
        reg.register(ListDirTool);
        reg.register(BashTool);
        reg.register(GrepTool);
        reg.register(GlobTool);
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

// ─── Grep (ripgrep when available, fallback to built-in) ───

struct GrepTool;

impl Tool for GrepTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "grep".into(),
            description: "Search for a pattern in files. Uses ripgrep (rg) when available for speed, falls back to built-in search. Respects .gitignore.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Search pattern (regex when using ripgrep, substring for fallback)" },
                    "path": { "type": "string", "description": "Directory to search in (default: project root)" },
                    "include": { "type": "string", "description": "File glob pattern to include (e.g., '*.rs', '*.py')" },
                    "fixed_strings": { "type": "boolean", "description": "Treat pattern as literal string, not regex (default: false)" }
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

        // Try ripgrep first
        if let Ok(_) = std::process::Command::new("rg").arg("--version").output() {
            let mut cmd = std::process::Command::new("rg");
            cmd.args([
                "--line-number",
                "--no-heading",
                "--color=never",
                "--max-count=200",
                "--max-columns=300",
                "--max-columns-preview",
            ]);
            if fixed {
                cmd.arg("--fixed-strings");
            }
            if let Some(glob) = include {
                cmd.args(["--glob", glob]);
            }
            cmd.arg(pattern);
            cmd.arg(&search_dir);

            match cmd.output() {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let result = if stdout.is_empty() {
                        format!("No matches for '{}'", pattern)
                    } else {
                        // Make paths relative
                        let prefix = format!("{}/", search_dir);
                        stdout
                            .lines()
                            .map(|l| l.strip_prefix(&prefix).unwrap_or(l))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    return ToolResult {
                        tool_use_id: String::new(),
                        content: result,
                        is_error: false,
                    };
                }
                Err(_) => {} // Fall through to built-in
            }
        }

        // Built-in fallback (case-insensitive substring search)
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

            // Include filter
            if let Some(glob) = include {
                let name = path.to_string_lossy();
                let glob_clean = glob.trim_start_matches('*');
                if !name.ends_with(glob_clean) {
                    continue;
                }
            }

            if let Ok(content) = std::fs::read_to_string(path) {
                let relative = path
                    .to_string_lossy()
                    .strip_prefix(&search_dir)
                    .unwrap_or(&path.to_string_lossy())
                    .trim_start_matches('/')
                    .to_string();
                for (i, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&pattern_lower) {
                        results.push(format!("{}:{}: {}", relative, i + 1, line.trim()));
                        if results.len() >= 200 {
                            break;
                        }
                    }
                }
            }
            if results.len() >= 200 {
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

// ─── Glob (find files by pattern) ───

struct GlobTool;

impl Tool for GlobTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "glob".into(),
            description: "Find files by name pattern. Uses fd when available for speed, falls back to built-in search. Respects .gitignore.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "File name pattern (substring match, e.g., 'test', '.rs', 'config')" },
                    "path": { "type": "string", "description": "Directory to search in (default: project root)" }
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

        // Try fd first
        if let Ok(_) = std::process::Command::new("fd").arg("--version").output() {
            let output = std::process::Command::new("fd")
                .args(["--color=never", "--max-results=100", pattern])
                .arg(&search_dir)
                .output();

            if let Ok(output) = output {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.is_empty() {
                    let prefix = format!("{}/", search_dir);
                    let result = stdout
                        .lines()
                        .map(|l| l.strip_prefix(&prefix).unwrap_or(l))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return ToolResult {
                        tool_use_id: String::new(),
                        content: result,
                        is_error: false,
                    };
                }
            }
        }

        // Built-in fallback
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
                if results.len() >= 100 {
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
