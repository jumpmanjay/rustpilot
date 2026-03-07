# RustPilot — Developer Cockpit TUI

> A single terminal application that replaces the multi-window workflow of VSCode + prompt editor + OpenCode.

## Problem

Current workflow requires three separate windows across two monitors:
1. VSCode (diff viewer, file explorer, workspace search)
2. Custom lightweight editor (prompt authoring, prompt cataloging)
3. OpenCode terminal (LLM streaming output)

The core bottleneck: can't write the next prompt while watching the LLM execute the current one.

## Solution

A Ratatui-based TUI with three flexible panels that can be shown/hidden, resized, and rearranged.

## Panels

### 1. Code Panel
- **File explorer** — tree view, expand/collapse directories
  - Keybind to send file/directory path to prompt manager
- **Editor** — nano-level, syntax highlighting, line numbers
  - Keybind to send `filepath:line` or `filepath:startline-endline` to prompt manager
  - Basic editing: insert, delete, copy, paste, undo
  - Save with keybind
- **Diff viewer** — side-by-side, color-coded hunks
  - Git diff of working tree, or diff between two files
- **Search** — workspace-wide grep with results list
  - Navigate to result → opens in editor
  - Keybind to send result path+line to prompt manager

### 2. LLM Panel
- **Streaming output** — real-time token display from LLM API
- **Tool call visibility** — show when the LLM reads files, runs commands, etc.
- **Status bar** — model name, token usage, cost estimate, elapsed time
- **Scrollback** — scroll up through output while new tokens stream below
- **Multiple sessions** — ability to have more than one LLM conversation active (tabs/selector)

### 3. Prompt Manager
- **Hierarchy**: Project → Thread → Prompt/Response pairs
- **Compose view** — write/edit prompts with multi-line editing
- **Quick-insert references** — receive `filepath:line` refs from other panels, insert as context
- **History browser** — scroll through past prompts and responses per thread
- **Search** — full-text search across all prompts/responses
- **Categories/tags** — organize projects, mark successful prompt patterns
- **Send prompt** — dispatches to LLM panel, streams response back

## Panel Management

- Any panel can be **shown or hidden** (toggle keybinds)
- Panels can be **rearranged** — horizontal or vertical splits, any order
- Panels are **resizable** — drag borders or keybind to adjust ratios
- **Layouts** — save/restore named layouts (e.g., "review mode" = code + diff, "prompt mode" = prompt + LLM)
- Sensible defaults: Code left, LLM top-right, Prompt bottom-right

## Path/Line Reference System

The "send to prompt" flow is a first-class interaction:
1. In **file explorer**: cursor on file/dir → keybind → path inserted into prompt compose
2. In **editor**: select line(s) → keybind → `path/to/file.rs:42` or `path/to/file.rs:42-58` inserted
3. In **search results**: keybind → result path+line inserted
4. In **prompt manager**: references are rendered as clickable/navigable links back to the code panel

### Reference Syntax
- `@path/to/file.rs:42-58` — **tag reference**: path+lines stored as-is, LLM reads the file at send time
- `@@path/to/file.rs:42-58` — **include reference**: file content from those lines is inlined into the prompt at send time
- Keybind from editor/explorer inserts as tag by default; modifier key (e.g., Ctrl+Shift) inserts as include

## LLM Integration

- Direct API calls (not wrapping OpenCode)
- Support for Anthropic Claude (primary), extensible to other providers
- Streaming via SSE/event streams
- Conversation context management per thread
- System prompt configuration per project
- Token counting and cost tracking

## Data Storage

NFS-safe by design — no SQLite, no file locking.

### Directory Structure
```
~/.rustpilot/
  config.toml                    # keybinds, API keys, preferences, layouts
  projects/
    my-project/
      project.json               # metadata, system prompt, settings
      threads/
        thread-abc123.jsonl      # one JSONL file per thread
        thread-def456.jsonl
```

### JSONL Format (compact, no pretty printing)
Each line is one entry:
```jsonl
{"id":"msg_01","ts":1709827200,"role":"user","content":"refactor the auth module","refs":["src/auth.rs:42-58","src/lib.rs:10"],"tags":["refactor"]}
{"id":"msg_02","ts":1709827215,"role":"assistant","content":"I'll refactor...","model":"claude-sonnet-4-20250514","tokens_in":1200,"tokens_out":3400,"cost_usd":0.012}
```

### Reference Types in Prompts
- **Tag reference**: `@src/auth.rs:42-58` — path+line marker, LLM resolves at send time
- **Include reference**: `@@src/auth.rs:42-58` — content is inlined into the prompt at send time

### Why JSONL
- Append-only, NFS-safe (no file locking needed)
- One file per thread keeps files small
- Trivially grep-able
- Easy to export/archive

- **Config** — TOML for keybinds, API keys, preferences, layouts
- **Workspace awareness** — respects `.gitignore`, understands project root

## Tech Stack

- **Language**: Rust
- **TUI framework**: Ratatui + Crossterm
- **Async runtime**: Tokio
- **HTTP/Streaming**: reqwest with streaming
- **Syntax highlighting**: tree-sitter or syntect
- **Storage**: JSONL (serde_json)
- **Diff engine**: similar (Rust diffing crate)
- **File watching**: notify (for live file updates)

## Keybind Philosophy

- Ctrl-based, eventually fully customizable
- Each panel has its own keybind context
- Global keybinds for panel management (Ctrl+1/2/3 to focus, Ctrl+` to cycle)
- Panel-local keybinds displayed in status bar

## MVP Scope (v0.1)

1. ✅ Panel framework — show/hide/resize three panels
2. ✅ File explorer — navigate, open files
3. ✅ Basic editor — view/edit files, syntax highlighting, line numbers
4. ✅ Prompt manager — project/thread hierarchy, compose, history
5. ✅ Path/line reference insertion (editor → prompt, explorer → prompt)
6. ✅ LLM streaming — single provider (Claude), single conversation
7. ✅ JSONL prompt storage

## Post-MVP

- Diff viewer
- Workspace search/grep
- Multiple LLM sessions
- Layout save/restore
- Git integration
- Cost tracking dashboard
- Prompt templates/snippets
- Export threads to markdown