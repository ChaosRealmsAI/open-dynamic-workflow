use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::io::{now_millis, pandacode_dir};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub runtime: String,
    pub session: String,
    pub driver: String,
    pub workspace: String,
    pub run_id: Option<String>,
    pub thread_id: Option<String>,
    pub thread_path: Option<String>,
    pub tmux_name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Option<String>,
    pub artifacts: serde_json::Value,
    pub created_ms: u128,
    pub updated_ms: u128,
}

impl SessionRecord {
    pub fn new(runtime: &str, session: &str, driver: &str, workspace: &Path) -> Self {
        let now = now_millis();
        Self {
            runtime: runtime.to_string(),
            session: session.to_string(),
            driver: driver.to_string(),
            workspace: workspace.to_string_lossy().to_string(),
            run_id: None,
            thread_id: None,
            thread_path: None,
            tmux_name: None,
            model: None,
            effort: None,
            permission: None,
            artifacts: json!({}),
            created_ms: now,
            updated_ms: now,
        }
    }
}

pub fn save(root: &Path, record: &mut SessionRecord) -> Result<()> {
    record.updated_ms = now_millis();
    let dir = runtime_dir(root, &record.runtime);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = record_path(root, &record.runtime, &record.session);
    write_atomic(
        &path,
        &format!("{}\n", serde_json::to_string_pretty(record)?),
    )?;
    let pointer = format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "runtime": record.runtime,
            "session": record.session,
            "updated_ms": record.updated_ms
        }))?
    );
    write_atomic(&latest_path(root, &record.runtime), &pointer)?;
    write_atomic(&global_latest_path(root), &pointer)?;
    Ok(())
}

// Write a file atomically (per-process tmp + rename) so two `pandacode` processes
// running in the same project dir can't interleave and leave a torn `latest.json`.
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, content).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

pub fn load(root: &Path, runtime: &str, session: &str) -> Result<SessionRecord> {
    let session = resolve_session(root, runtime, session)?;
    let path = record_path(root, runtime, &session);
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

pub fn resolve_session(root: &Path, runtime: &str, requested: &str) -> Result<String> {
    if requested != "latest" {
        return Ok(requested.to_string());
    }
    let path = latest_path(root, runtime);
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "no latest {runtime} session at {}; run `pandacode {runtime} exec ...` first or pass --session",
            path.display()
        )
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    value
        .get("session")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("{} does not contain session", path.display()))
}

pub fn resolve_global_latest(root: &Path) -> Result<(String, String)> {
    let path = global_latest_path(root);
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "no latest PandaCode session at {}; run `pandacode run ...` first or pass --runtime",
            path.display()
        )
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let runtime = value
        .get("runtime")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("{} does not contain runtime", path.display()))?;
    let session = value
        .get("session")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("{} does not contain session", path.display()))?;
    Ok((runtime.to_string(), session.to_string()))
}

pub fn resolve_runtime_for_session(root: &Path, requested: &str) -> Result<String> {
    if requested == "latest" {
        return resolve_global_latest(root).map(|(runtime, _session)| runtime);
    }

    let runtimes = ["bamboo", "claude", "codex"];
    let mut matches = Vec::new();
    for runtime in runtimes {
        if record_path(root, runtime, requested).exists() {
            matches.push(runtime);
        }
    }

    match matches.as_slice() {
        [runtime] => Ok((*runtime).to_string()),
        [] => Err(anyhow!(
            "no PandaCode session {requested:?}; pass --runtime if it belongs to a runtime-specific store or run `pandacode list --json`"
        )),
        many => Err(anyhow!(
            "session {requested:?} exists in multiple runtimes ({});
pass --runtime bamboo|claude|codex",
            many.join(", ")
        )),
    }
}

pub fn list(root: &Path, runtime: &str) -> Result<Vec<SessionRecord>> {
    let dir = runtime_dir(root, runtime);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some("latest.json") {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path)
            && let Ok(record) = serde_json::from_str::<SessionRecord>(&text)
        {
            records.push(record);
        }
    }
    records.sort_by_key(|record| record.updated_ms);
    records.reverse();
    Ok(records)
}

pub fn artifacts(root: &Path, runtime: &str, session: &str) -> Result<serde_json::Value> {
    let record = load(root, runtime, session)?;
    Ok(json!({
        "ok": true,
        "state": "available",
        "runtime": runtime,
        "action": "artifacts",
        "session": record.session,
        "workspace": record.workspace,
        "record_path": record_path(root, runtime, &record.session),
        "artifacts": record.artifacts,
        "record": record
    }))
}

fn runtime_dir(root: &Path, runtime: &str) -> PathBuf {
    pandacode_dir(root).join("sessions").join(runtime)
}

fn record_path(root: &Path, runtime: &str, session: &str) -> PathBuf {
    // Sanitize the session into a single filename component (same as the exec
    // path does) so a `--session ../../x` can't read/write outside the runtime
    // dir — separators are stripped, so the result can never traverse.
    let safe = crate::io::sanitize_name(session, runtime);
    runtime_dir(root, runtime).join(format!("{safe}.json"))
}

fn latest_path(root: &Path, runtime: &str) -> PathBuf {
    runtime_dir(root, runtime).join("latest.json")
}

fn global_latest_path(root: &Path) -> PathBuf {
    pandacode_dir(root).join("sessions").join("latest.json")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn saves_and_resolves_latest() {
        let root = std::env::temp_dir().join(format!("pandacode-session-test-{}", now_millis()));
        fs::create_dir_all(&root).unwrap();
        let mut record = SessionRecord::new("codex", "s1", "codex-appserver", &root);
        save(&root, &mut record).unwrap();
        assert_eq!(resolve_session(&root, "codex", "latest").unwrap(), "s1");
        assert_eq!(load(&root, "codex", "latest").unwrap().session, "s1");
        assert_eq!(
            resolve_global_latest(&root).unwrap(),
            ("codex".to_string(), "s1".to_string())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_latest_is_clear() {
        let root = std::env::temp_dir().join(format!("pandacode-session-missing-{}", now_millis()));
        fs::create_dir_all(&root).unwrap();
        let error = resolve_session(&root, "claude", "latest").unwrap_err();
        assert!(error.to_string().contains("no latest claude session"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_path_sanitizes_session_no_traversal() {
        let root = Path::new("/project");
        let dir = runtime_dir(root, "claude");
        let p = record_path(root, "claude", "../../../etc/passwd");
        // Stays inside the runtime dir; separators stripped, so no traversal.
        assert!(p.starts_with(&dir), "escaped runtime dir: {}", p.display());
        assert!(!p.to_string_lossy().contains("/etc/passwd"));
        // A normal session still maps to <session>.json under the runtime dir.
        assert_eq!(
            record_path(root, "claude", "abc-1.2"),
            dir.join("abc-1.2.json")
        );
    }
}
