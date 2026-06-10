//! Minimal direct client for `codex app-server` over stdio JSONL.
//!
//! PandaCode spawns one app-server process per turn, drives it with JSON-RPC
//! requests (`thread/start`, `thread/resume`, `turn/start`), and reads event
//! notifications until the turn completes. Thread state persists in the Codex
//! home as rollout files, so multi-turn sessions resume across processes
//! without a daemon.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

pub struct AppServerClient {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    rx: Receiver<ServerLine>,
    next_id: u64,
    log_file: Option<std::fs::File>,
    pid_file: Option<PathBuf>,
}

enum ServerLine {
    Stdout(String),
    Stderr(String),
}

impl AppServerClient {
    pub fn spawn(
        bins: &crate::cli::RuntimeBins,
        cwd: &Path,
        log_path: Option<&Path>,
    ) -> Result<Self> {
        let mut command = Command::new(&bins.codex_bin);
        command
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match effective_codex_home(bins.auth_home.as_deref(), bins.codex_home.as_deref()) {
            Ok(home) => {
                command.env("CODEX_HOME", &home);
            }
            Err(error) => {
                // Fall back to the caller environment rather than failing the
                // turn over auth-copy housekeeping.
                eprintln!("pandacode: managed codex home unavailable ({error}); using default CODEX_HOME");
            }
        }
        // Own process group so kill() can reap wrapper grandchildren too
        // (`codex` is often a node wrapper that re-spawns the real binary).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {} app-server", bins.codex_bin))?;
        let stdin = child.stdin.take().context("app-server stdin missing")?;
        let stdout = child.stdout.take().context("app-server stdout missing")?;
        let stderr = child.stderr.take().context("app-server stderr missing")?;
        let (tx, rx) = mpsc::channel();
        let tx_stdout = tx.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let _ = tx_stdout.send(ServerLine::Stdout(line));
            }
        });
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let _ = tx.send(ServerLine::Stderr(line));
            }
        });
        let log_file = match log_path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                Some(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?,
                )
            }
            None => None,
        };
        let pid_file = pid_dir(cwd).join(child.id().to_string());
        if let Some(parent) = pid_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&pid_file, b"");
        Ok(Self {
            child,
            stdin: Some(BufWriter::new(stdin)),
            rx,
            next_id: 1,
            log_file,
            pid_file: Some(pid_file),
        })
    }

    pub fn initialize(&mut self, timeout: Duration) -> Result<Value> {
        let id = self.send_request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "pandacode",
                    "title": "PandaCode CLI",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                    "optOutNotificationMethods": ["fs/changed"],
                },
            }),
        )?;
        let response = self.wait_response(id, timeout)?;
        if let Some(error) = response.get("error") {
            bail!("initialize failed: {error}");
        }
        self.send_notification("initialized", None)?;
        Ok(response)
    }

    pub fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.send_request(method, params)?;
        let response = self.wait_response(id, timeout)?;
        if let Some(error) = response.get("error") {
            bail!("{method} failed: {error}");
        }
        Ok(response)
    }

    pub fn send_request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_value(json!({ "id": id, "method": method, "params": params }))?;
        Ok(id)
    }

    pub fn send_response(&mut self, id: Value, result: Value) -> Result<()> {
        self.send_value(json!({ "id": id, "result": result }))
    }

    fn send_notification(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let value = match params {
            Some(params) => json!({ "method": method, "params": params }),
            None => json!({ "method": method }),
        };
        self.send_value(value)
    }

    fn send_value(&mut self, value: Value) -> Result<()> {
        self.log("out", &value);
        let stdin = self
            .stdin
            .as_mut()
            .context("app-server stdin already closed")?;
        writeln!(stdin, "{value}")?;
        stdin.flush()?;
        Ok(())
    }

    fn wait_response(&mut self, id: u64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let message = self.recv_until(deadline)?;
            if is_response_with_id(&message, id) {
                return Ok(message);
            }
        }
    }

    /// Receive one message, returning Ok(None) when `timeout` elapses first.
    pub fn recv_maybe(&mut self, timeout: Duration) -> Result<Option<Value>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            let line = match self.rx.recv_timeout(remaining) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => bail!("app-server stream closed"),
            };
            match line {
                ServerLine::Stdout(line) => {
                    let value: Value = serde_json::from_str(&line)
                        .with_context(|| format!("app-server returned invalid JSON: {line}"))?;
                    self.log("in", &value);
                    return Ok(Some(value));
                }
                ServerLine::Stderr(line) => {
                    self.log("stderr", &json!({ "line": line }));
                }
            }
        }
    }

    pub fn recv_until(&mut self, deadline: Instant) -> Result<Value> {
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                bail!("timed out waiting for app-server message");
            }
            let line = match self.rx.recv_timeout(remaining) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    bail!("timed out waiting for app-server message")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("app-server stream closed")
                }
            };
            match line {
                ServerLine::Stdout(line) => {
                    let value: Value = serde_json::from_str(&line)
                        .with_context(|| format!("app-server returned invalid JSON: {line}"))?;
                    self.log("in", &value);
                    return Ok(value);
                }
                ServerLine::Stderr(line) => {
                    self.log("stderr", &json!({ "line": line }));
                }
            }
        }
    }

    pub fn kill(&mut self) {
        // Close stdin first so a well-behaved server exits on EOF.
        drop(self.stdin.take());
        let pid = self.child.id();
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The child owns its process group (see spawn), so a group kill reaps
        // wrapper grandchildren such as the npm codex shim's vendor binary.
        kill_process_group(pid);
        if let Some(pid_file) = self.pid_file.take() {
            let _ = std::fs::remove_file(pid_file);
        }
    }

    fn log(&mut self, direction: &str, value: &Value) {
        if let Some(file) = self.log_file.as_mut() {
            let entry = json!({
                "ms": crate::io::now_millis(),
                "dir": direction,
                "msg": value,
            });
            let _ = writeln!(file, "{entry}");
        }
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        self.kill();
    }
}

fn is_response_with_id(message: &Value, id: u64) -> bool {
    message.get("method").is_none()
        && message
            .get("id")
            .and_then(Value::as_u64)
            .is_some_and(|value| value == id)
}

/// Whether a message is a server-initiated request (has both id and method).
pub fn server_request_id(message: &Value) -> Option<Value> {
    if message.get("method").is_some() {
        message.get("id").cloned()
    } else {
        None
    }
}

pub fn notification_method(message: &Value) -> Option<&str> {
    if message.get("id").is_none() {
        message.get("method").and_then(Value::as_str)
    } else {
        None
    }
}

pub fn spawn_initialized(
    bins: &crate::cli::RuntimeBins,
    cwd: &Path,
    log_path: Option<PathBuf>,
) -> Result<AppServerClient> {
    reap_orphans(cwd);
    let mut client = AppServerClient::spawn(bins, cwd, log_path.as_deref())
        .map_err(|error| anyhow!("{error}"))?;
    client.initialize(Duration::from_secs(30))?;
    Ok(client)
}

fn pid_dir(root: &Path) -> PathBuf {
    crate::io::pandacode_dir(root)
        .join("codex")
        .join("appserver-pids")
}

/// Resolve the Codex home for a turn. Precedence:
/// 1. `--codex-home DIR` (or PANDACODE_CODEX_HOME): use the full home as-is.
/// 2. Managed clean home: copy auth material from `--auth-home DIR` (or
///    CODEX_HOME env, or `~/.codex`) into a per-account directory under
///    `~/.pandacode/codex-home/`, deliberately leaving out config.toml,
///    AGENTS.md, and skills so turns run with a clean configuration.
pub fn effective_codex_home(
    auth_home: Option<&Path>,
    full_home: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(full) = full_home {
        return Ok(full.to_path_buf());
    }
    if let Some(custom) = std::env::var_os("PANDACODE_CODEX_HOME") {
        return Ok(PathBuf::from(custom));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let source = auth_home
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .unwrap_or_else(|| home.join(".codex"));
    let account = managed_account_name(&source, &home);
    let target = home.join(".pandacode").join("codex-home").join(account);
    std::fs::create_dir_all(&target)
        .with_context(|| format!("create managed codex home {}", target.display()))?;
    for name in ["config.toml", "AGENTS.md", "skills"] {
        let path = target.join(name);
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
    for name in ["auth.json", "installation_id", "version.json"] {
        let from = source.join(name);
        if from.exists() {
            let _ = std::fs::copy(&from, target.join(name));
        }
    }
    Ok(target)
}

fn managed_account_name(source: &Path, home: &Path) -> String {
    if source == home.join(".codex") {
        return "default".to_string();
    }
    source
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

pub fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-9", "--", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Best-effort reaper for app-server process groups left behind when a prior
/// pandacode process died without running destructors (e.g. SIGKILL). Every
/// spawn records its pid under `.pandacode/codex/appserver-pids/`; an entry is
/// an orphan only when the process is still an `app-server` AND has been
/// re-parented to init (ppid 1) — a live pandacode parent means the entry
/// belongs to a concurrent run and must be left alone.
pub fn reap_orphans(root: &Path) {
    let dir = pid_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(pid) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u32>().ok())
        else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let line = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "ppid=,command="])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
            .unwrap_or_default();
        let line = line.trim();
        if line.is_empty() {
            // Process is gone; the entry is stale bookkeeping.
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let ppid = line
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u32>().ok());
        let orphaned_appserver = ppid == Some(1) && line.contains("app-server");
        if orphaned_appserver {
            kill_process_group(pid);
            let _ = std::fs::remove_file(&path);
        } else if !line.contains("app-server") {
            // Pid recycled by an unrelated process: drop the stale entry.
            let _ = std::fs::remove_file(&path);
        }
        // Otherwise: a live app-server owned by a concurrent pandacode run —
        // leave both the process and its entry untouched.
    }
}
