use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde_json::json;

pub const TASK_FILE_REFERENCE_THRESHOLD_BYTES: usize = 6 * 1024;
pub const STRUCTURED_LOG_TAIL_MAX_CHARS: usize = 16 * 1024;
pub const PANDACODE_STATE_DIR_ENV: &str = "PANDACODE_STATE_DIR";

#[derive(Debug)]
pub struct JsonAlreadyEmitted;

impl std::fmt::Display for JsonAlreadyEmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON response already emitted")
    }
}

impl std::error::Error for JsonAlreadyEmitted {}

pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub fn workspace(path: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let normalized = candidate
        .canonicalize()
        .with_context(|| format!("resolve workspace {}", candidate.display()))?;
    if !normalized.is_dir() {
        bail!("workspace is not a directory: {}", normalized.display());
    }
    Ok(normalized)
}

pub fn pandacode_dir(root: &Path) -> PathBuf {
    let override_dir = std::env::var_os(PANDACODE_STATE_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    pandacode_dir_with_override(root, override_dir.as_deref())
}

pub(crate) fn pandacode_dir_with_override(root: &Path, override_dir: Option<&Path>) -> PathBuf {
    match override_dir {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => root.join(path),
        None => root.join(".pandacode"),
    }
}

pub fn read_task(
    task: Option<&str>,
    task_file: Option<&Path>,
    stdin_marker: Option<&str>,
    workspace_root: Option<&Path>,
) -> Result<String> {
    match (task, task_file, stdin_marker) {
        (Some(task), None, None) => Ok(task.to_string()),
        (None, Some(path), None) => read_task_file(path, workspace_root),
        (None, None, Some("-")) => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            Ok(input)
        }
        (None, None, Some(other)) => bail!("unsupported positional task marker: {other}; use '-'"),
        (None, None, None) => bail!("pass --task, --task-file, or '-' to read task from stdin"),
        _ => bail!("pass exactly one task source: --task, --task-file, or '-'"),
    }
}

fn read_task_file(path: &Path, workspace_root: Option<&Path>) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(original_error) => {
            if path.is_absolute() {
                return Err(original_error)
                    .with_context(|| format!("read task file {}", path.display()));
            }
            if let Some(root) = workspace_root {
                let candidate = root.join(path);
                if candidate != path {
                    return fs::read_to_string(&candidate).with_context(|| {
                        format!(
                            "read task file {} or {}",
                            path.display(),
                            candidate.display()
                        )
                    });
                }
            }
            Err(original_error).with_context(|| format!("read task file {}", path.display()))
        }
    }
}

pub fn write_prompt_file(root: &Path, runtime: &str, session: &str, task: &str) -> Result<PathBuf> {
    let dir = pandacode_dir(root).join(runtime).join("prompts");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{session}-{}.md", now_millis()));
    fs::write(&path, task).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn dispatch_task_for_transport(task: &str, prompt_file: &Path) -> Option<String> {
    if task.len() <= TASK_FILE_REFERENCE_THRESHOLD_BYTES {
        return None;
    }
    Some(format!(
        "The full task prompt is stored in this local file:\n{}\n\nRead that file first, then execute its contents exactly as the user task. Do not summarize the file instead of doing the work. If the file asks for a specific final response format or schema, obey that final response contract after completing the task.",
        prompt_file.display()
    ))
}

#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_capture(command: &[String], cwd: Option<&Path>) -> Result<CmdOutput> {
    let Some((program, args)) = command.split_first() else {
        bail!("empty command");
    };
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd.output().with_context(|| format!("run {program}"))?;
    Ok(CmdOutput {
        ok: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub fn run_status(command: &[String], cwd: Option<&Path>) -> Result<CmdOutput> {
    run_capture(command, cwd)
}

pub fn run_stdin(command: &[String], input: &[u8], cwd: Option<&Path>) -> Result<CmdOutput> {
    let Some((program, args)) = command.split_first() else {
        bail!("empty command");
    };
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.spawn().with_context(|| format!("run {program}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    Ok(CmdOutput {
        ok: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub fn output_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn command_report(
    ok: bool,
    runtime: &str,
    action: &str,
    session: Option<&str>,
    command: &[String],
    output: &CmdOutput,
    raw: Option<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "ok": ok,
        "runtime": runtime,
        "action": action,
        "session": session,
        "command": command,
        "shell": shell_join(command),
        "exit_code": output.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
        "raw": raw
    })
}

pub fn parse_json_or_null(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(text.trim()).ok()
}

pub fn tail(text: &str, lines: usize) -> String {
    let mut tail = text.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    tail.join("\n")
}

pub fn structured_log_tail(text: &str, lines: usize) -> String {
    let cleaned = strip_ansi_controls(text);
    let tailed = tail(&cleaned, lines);
    truncate_chars(
        &redact_sensitive_text(&tailed),
        STRUCTURED_LOG_TAIL_MAX_CHARS,
    )
}

pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("\n[truncated]");
    }
    out
}

fn redact_sensitive_text(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.split_whitespace()
                .map(redact_sensitive_token)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_sensitive_token(token: &str) -> String {
    let trimmed = token.trim_matches(|ch: char| {
        matches!(
            ch,
            ',' | ';' | ':' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    });
    if looks_like_api_key(trimmed) {
        return mask_token(token, trimmed);
    }
    if looks_like_email(trimmed) {
        return token.replace(trimmed, "[redacted-email]");
    }
    token.to_string()
}

fn looks_like_api_key(token: &str) -> bool {
    token.len() >= 16
        && (token.starts_with("sk-")
            || token.starts_with("ak-")
            || token.starts_with("sk_")
            || token.starts_with("key-"))
}

fn looks_like_email(token: &str) -> bool {
    token.contains('@')
        && token.contains('.')
        && !token.contains('/')
        && !token.starts_with('@')
        && !token.ends_with('@')
}

fn mask_token(original: &str, token: &str) -> String {
    let prefix = token.chars().take(6).collect::<String>();
    let suffix = token
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    original.replace(token, &format!("{prefix}***{suffix}"))
}

pub fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| shell_quote(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub fn strip_ansi_controls(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    let mut previous_escape = false;
                    for next in chars.by_ref() {
                        if next == '\u{7}' || (previous_escape && next == '\\') {
                            break;
                        }
                        previous_escape = next == '\u{1b}';
                    }
                }
                _ => {
                    chars.next();
                }
            }
            continue;
        }
        if ch.is_control() && !matches!(ch, '\n' | '\r' | '\t') {
            continue;
        }
        out.push(ch);
    }
    out
}

pub fn sanitize_name(raw: &str, fallback_prefix: &str) -> String {
    let normalized = raw
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if normalized.is_empty() {
        format!("{fallback_prefix}-{}-{}", now_millis(), std::process::id())
    } else {
        normalized
    }
}

pub fn generated_session(runtime: &str) -> String {
    format!("{runtime}-{}-{}", now_millis(), std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names_and_quotes_shell() {
        assert_eq!(sanitize_name("main/review", "x"), "main-review");
        assert_eq!(shell_join(&["a b".to_string(), "c".to_string()]), "'a b' c");
    }

    #[test]
    fn strips_ansi_controls() {
        assert_eq!(strip_ansi_controls("\u{1b}[31mred\u{1b}[0m"), "red");
    }

    #[test]
    fn long_tasks_are_dispatched_by_file_reference() {
        let path = PathBuf::from("/tmp/pandacode-task.md");
        assert!(dispatch_task_for_transport("short", &path).is_none());
        let long = "x".repeat(TASK_FILE_REFERENCE_THRESHOLD_BYTES + 1);
        let dispatch = dispatch_task_for_transport(&long, &path).unwrap();
        assert!(dispatch.contains("/tmp/pandacode-task.md"));
        assert!(dispatch.contains("Read that file first"));
    }

    #[test]
    fn pandacode_state_dir_override_resolves_relative_to_workspace() {
        let root = Path::new("/repo");
        assert_eq!(
            pandacode_dir_with_override(root, None),
            PathBuf::from("/repo/.pandacode")
        );
        assert_eq!(
            pandacode_dir_with_override(root, Some(Path::new(".odw/runs/r1/pandacode-state"))),
            PathBuf::from("/repo/.odw/runs/r1/pandacode-state")
        );
        assert_eq!(
            pandacode_dir_with_override(root, Some(Path::new("/tmp/panda-state"))),
            PathBuf::from("/tmp/panda-state")
        );
    }

    #[test]
    fn task_file_can_resolve_relative_to_workspace_root() {
        let root = std::env::temp_dir().join(format!(
            "pandacode-task-file-root-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(root.join("task.md"), "workspace task").unwrap();

        let task = read_task(
            None,
            Some(Path::new("../task.md")),
            None,
            Some(&root.join("app")),
        )
        .unwrap();
        assert_eq!(task, "workspace task");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn structured_log_tail_redacts_common_secrets() {
        let text = "hello user@example.com\nkey sk-1234567890abcdef\n";
        let tail = structured_log_tail(text, 10);
        assert!(tail.contains("[redacted-email]"));
        assert!(tail.contains("sk-123***cdef"));
        assert!(!tail.contains("user@example.com"));
        assert!(!tail.contains("sk-1234567890abcdef"));
    }
}
