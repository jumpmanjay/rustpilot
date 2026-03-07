# RustPilot

Developer cockpit TUI — code editor, LLM streaming, and prompt manager in one terminal.

## Quick Start

```bash
cargo run
```

## Configuration

RustPilot creates `~/.rustpilot/config.toml` on first run. Set your API key:

```toml
[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "sk-ant-..."
max_tokens = 8192
```

## Keybinds

| Key | Action |
|-----|--------|
| `Ctrl+1` | Toggle Code panel |
| `Ctrl+2` | Toggle LLM panel |
| `Ctrl+3` | Toggle Prompt panel |
| `Ctrl+`` ` | Cycle focus between panels |
| `Ctrl+Q` | Quit |

### Code Panel (Explorer)
| Key | Action |
|-----|--------|
| `j/k` or `↑/↓` | Navigate |
| `Enter` | Open file/directory |
| `Backspace` | Go up a directory |
| `Ctrl+R` | Send path as tag reference (`@path`) |
| `Ctrl+Shift+R` | Send path as include reference (`@@path`) |

### Code Panel (Editor)
| Key | Action |
|-----|--------|
| `Ctrl+S` | Save |
| `Ctrl+E` | Back to explorer |
| `Ctrl+L` | Toggle line selection |
| `Ctrl+R` | Send line ref to prompt (`@path:line`) |
| `Ctrl+Shift+R` | Send as include ref (`@@path:line`) |

### Prompt Panel
| Key | Action |
|-----|--------|
| `Ctrl+N` | New project/thread |
| `Ctrl+Enter` | Send prompt to LLM |
| `Ctrl+H` | View thread history |
| `Esc` | Back |

See [SPEC.md](SPEC.md) for the full design document.
