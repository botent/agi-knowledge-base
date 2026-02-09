# Auto-Agent Recipes

Memini supports file-backed background agents in:

- `$MEMINI_HOME/agents`
- default: `~/Memini/agents`

Use `/auto dir` to print the active directory path.

## Recipe Format

Each recipe is a Markdown file with front matter and instructions body:

```md
---
name: repo-watch
description: keep an eye on repo health
interval_secs: 1800
auto_start: true
tools: workspace_list_files,workspace_read_file,workspace_run_command
persona: You are a repository watchdog agent.
---
Inspect the repository, run fast verification checks, and summarize failures, risky changes, and next actions.
```

## Supported Front Matter Keys

| Key | Required | Notes |
| --- | --- | --- |
| `name` | no | Defaults to filename stem |
| `description` | no | For `/auto` list output |
| `interval_secs` | no | Default `1800` |
| `auto_start` | no | `true` starts automatically on app launch |
| `tools` | no | Comma list. Use `local` for all workspace tools, `none` for no tools, or specific names |
| `persona` | no | System persona for this background agent |
| `instructions` | no | Alternative to markdown body |

If markdown body is non-empty, it is used as instructions.

## CLI Shortcuts

- `/auto create <name> <seconds> <instructions>`
- `/auto templates`
- `/auto scaffold <template> [name]`
- `/auto reload`
- `/auto start <name>`
- `/auto stop <name>`
- `/auto run <name>`
- `/auto remove <name>`
