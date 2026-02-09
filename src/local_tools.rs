//! Local workspace tools for autonomous agent actions.
//!
//! These tools let spawned agents inspect and modify files in the current
//! workspace, and run shell commands in that workspace.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::time::{Duration, timeout};

use crate::openai::ToolCall;

const MAX_LIST_ENTRIES: usize = 1000;
const MAX_READ_CHARS: usize = 50_000;
const MAX_COMMAND_TIMEOUT_SECS: u64 = 300;
const MAX_OUTPUT_CHARS: usize = 12_000;

pub fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "name": "workspace_list_files",
            "description": "List files and directories under the local workspace root. Use this before reading/writing files.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path inside the workspace. Defaults to '.'."
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "When true, traverse subdirectories recursively."
                    },
                    "max_entries": {
                        "type": "integer",
                        "description": "Max number of entries to return (default 200, max 1000)."
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "name": "workspace_read_file",
            "description": "Read a UTF-8 text file from the local workspace.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file inside the workspace."
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Maximum characters to return (default 20000, max 50000)."
                    }
                },
                "required": ["path"]
            }
        }),
        json!({
            "type": "function",
            "name": "workspace_write_file",
            "description": "Create or overwrite a text file in the local workspace.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to write."
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete file content to write."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "When false, fail if file already exists."
                    },
                    "create_parents": {
                        "type": "boolean",
                        "description": "When true, create missing parent directories."
                    }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "type": "function",
            "name": "workspace_run_command",
            "description": "Run a shell command in the local workspace and return exit code/stdout/stderr.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to run."
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Optional relative working directory inside the workspace."
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Timeout seconds (default 60, max 300)."
                    }
                },
                "required": ["command"]
            }
        }),
    ]
}

pub async fn handle_tool_call(call: &ToolCall) -> Option<String> {
    let output = match call.name.as_str() {
        "workspace_list_files" => to_output(handle_workspace_list_files(&call.arguments)),
        "workspace_read_file" => to_output(handle_workspace_read_file(&call.arguments)),
        "workspace_write_file" => to_output(handle_workspace_write_file(&call.arguments)),
        "workspace_run_command" => to_output(handle_workspace_run_command(&call.arguments).await),
        _ => return None,
    };
    Some(output)
}

fn handle_workspace_list_files(args: &Value) -> Result<Value> {
    let path_arg = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_entries = args
        .get("max_entries")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, MAX_LIST_ENTRIES as u64) as usize;

    let (workspace_root, target) = resolve_workspace_path(path_arg)?;
    if !target.exists() {
        bail!("Path does not exist: {}", target.display());
    }
    if !target.is_dir() {
        bail!("Path is not a directory: {}", target.display());
    }

    let mut entries = Vec::new();
    if recursive {
        let mut queue = VecDeque::new();
        queue.push_back(target.clone());
        while let Some(current) = queue.pop_front() {
            for dir_entry in fs::read_dir(&current)
                .with_context(|| format!("Read directory {}", current.display()))?
            {
                let dir_entry = dir_entry?;
                let file_type = dir_entry.file_type()?;
                let path = dir_entry.path();
                let entry = build_entry(&workspace_root, &path, file_type.is_dir())?;
                entries.push(entry);
                if entries.len() >= max_entries {
                    break;
                }
                if file_type.is_dir() {
                    queue.push_back(path);
                }
            }
            if entries.len() >= max_entries {
                break;
            }
        }
    } else {
        for dir_entry in
            fs::read_dir(&target).with_context(|| format!("Read directory {}", target.display()))?
        {
            let dir_entry = dir_entry?;
            let file_type = dir_entry.file_type()?;
            let entry = build_entry(&workspace_root, &dir_entry.path(), file_type.is_dir())?;
            entries.push(entry);
            if entries.len() >= max_entries {
                break;
            }
        }
    }

    Ok(json!({
        "workspace_root": workspace_root.display().to_string(),
        "path": to_workspace_relative(&target, &workspace_root),
        "entries": entries,
        "truncated": entries.len() >= max_entries,
    }))
}

fn build_entry(workspace_root: &Path, path: &Path, is_dir_hint: bool) -> Result<Value> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("Stat {}", path.display()))?;
    let file_type = meta.file_type();
    let kind = if file_type.is_dir() || is_dir_hint {
        "dir"
    } else if file_type.is_file() {
        "file"
    } else if file_type.is_symlink() {
        "symlink"
    } else {
        "other"
    };
    Ok(json!({
        "path": to_workspace_relative(path, workspace_root),
        "kind": kind,
        "size_bytes": meta.len(),
    }))
}

fn handle_workspace_read_file(args: &Value) -> Result<Value> {
    let path_arg = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("path is required"))?;
    let max_chars = args
        .get("max_chars")
        .and_then(Value::as_u64)
        .unwrap_or(20_000)
        .clamp(100, MAX_READ_CHARS as u64) as usize;

    let (workspace_root, path) = resolve_workspace_path(path_arg)?;
    if !path.exists() {
        bail!("File does not exist: {}", path.display());
    }
    if path.is_dir() {
        bail!("Path is a directory: {}", path.display());
    }

    let bytes = fs::read(&path).with_context(|| format!("Read {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let truncated = text.chars().count() > max_chars;
    let content = if truncated {
        format!(
            "{}\n...[truncated]",
            text.chars().take(max_chars).collect::<String>()
        )
    } else {
        text
    };

    Ok(json!({
        "path": to_workspace_relative(&path, &workspace_root),
        "size_bytes": bytes.len(),
        "truncated": truncated,
        "content": content,
    }))
}

fn handle_workspace_write_file(args: &Value) -> Result<Value> {
    let path_arg = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("path is required"))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("content is required"))?;
    let overwrite = args
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let create_parents = args
        .get("create_parents")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let (workspace_root, path) = resolve_workspace_path(path_arg)?;
    if path.exists() && !overwrite {
        bail!("Refusing to overwrite existing file: {}", path.display());
    }

    if let Some(parent) = path.parent() {
        if create_parents {
            fs::create_dir_all(parent).with_context(|| format!("Create {}", parent.display()))?;
        } else if !parent.exists() {
            bail!("Parent directory does not exist: {}", parent.display());
        }
    }

    fs::write(&path, content.as_bytes()).with_context(|| format!("Write {}", path.display()))?;

    Ok(json!({
        "path": to_workspace_relative(&path, &workspace_root),
        "bytes_written": content.len(),
        "status": "ok",
    }))
}

async fn handle_workspace_run_command(args: &Value) -> Result<Value> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("command is required"))?;
    let workdir_arg = args.get("workdir").and_then(Value::as_str).unwrap_or(".");
    let timeout_seconds = args
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(60)
        .clamp(1, MAX_COMMAND_TIMEOUT_SECS);

    let (workspace_root, workdir) = resolve_workspace_path(workdir_arg)?;
    if !workdir.exists() {
        bail!("Working directory does not exist: {}", workdir.display());
    }
    if !workdir.is_dir() {
        bail!(
            "Working directory is not a directory: {}",
            workdir.display()
        );
    }

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-lc")
        .arg(command)
        .current_dir(&workdir)
        .kill_on_drop(true);

    let timed = timeout(Duration::from_secs(timeout_seconds), cmd.output()).await;
    let output = match timed {
        Ok(result) => result.context("Run command")?,
        Err(_) => {
            return Ok(json!({
                "command": command,
                "workdir": to_workspace_relative(&workdir, &workspace_root),
                "timed_out": true,
                "timeout_seconds": timeout_seconds,
                "exit_code": Value::Null,
                "stdout": "",
                "stderr": "Command timed out.",
            }));
        }
    };

    let stdout = trim_chars(&String::from_utf8_lossy(&output.stdout), MAX_OUTPUT_CHARS);
    let stderr = trim_chars(&String::from_utf8_lossy(&output.stderr), MAX_OUTPUT_CHARS);

    Ok(json!({
        "command": command,
        "workdir": to_workspace_relative(&workdir, &workspace_root),
        "timed_out": false,
        "exit_code": output.status.code(),
        "success": output.status.success(),
        "stdout": stdout,
        "stderr": stderr,
    }))
}

fn to_output(result: Result<Value>) -> String {
    let payload = match result {
        Ok(value) => value,
        Err(err) => json!({ "error": err.to_string() }),
    };
    serde_json::to_string(&payload)
        .unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

fn resolve_workspace_path(raw: &str) -> Result<(PathBuf, PathBuf)> {
    let workspace_root = workspace_root();
    let target = if raw.trim().is_empty() {
        workspace_root.clone()
    } else {
        let input = PathBuf::from(raw.trim());
        if input.is_absolute() {
            normalize_path(&input)
        } else {
            normalize_path(&workspace_root.join(input))
        }
    };

    if !target.starts_with(&workspace_root) {
        bail!(
            "Path escapes workspace root. root={} path={}",
            workspace_root.display(),
            target.display()
        );
    }

    Ok((workspace_root, target))
}

fn workspace_root() -> PathBuf {
    if let Ok(raw) = env::var("MEMINI_WORKSPACE_ROOT") {
        if !raw.trim().is_empty() {
            let path = PathBuf::from(raw.trim());
            let absolute = if path.is_absolute() {
                path
            } else if let Ok(cwd) = env::current_dir() {
                cwd.join(path)
            } else {
                path
            };
            return normalize_path(&absolute);
        }
    }

    match env::current_dir() {
        Ok(cwd) => normalize_path(&cwd),
        Err(_) => PathBuf::from("."),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(seg) => normalized.push(seg),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn to_workspace_relative(path: &Path, workspace_root: &Path) -> String {
    match path.strip_prefix(workspace_root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

fn trim_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    format!(
        "{}\n...[truncated]",
        input.chars().take(max_chars).collect::<String>()
    )
}
