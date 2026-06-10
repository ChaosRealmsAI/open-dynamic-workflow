use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_pandacode")
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pandacode-{name}-{}-{}",
        std::process::id(),
        now_millis()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

#[test]
fn run_help_explains_common_task_options() {
    let output = Command::new(bin())
        .args(["run", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "Inline task text",
        "Read task text from a file",
        "Workspace directory",
        "Runtime to use",
        "Print machine-readable JSON",
        "Wait timeout in milliseconds",
    ] {
        assert!(
            help.contains(expected),
            "missing help text {expected}: {help}"
        );
    }
}

#[test]
fn top_level_list_defaults_to_compact_text_and_json_remains_machine_readable() {
    let root = temp_root("list-compact");

    let output = Command::new(bin())
        .args(["list", "--cd", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("PandaCode sessions"), "{text}");
    assert!(text.contains("bamboo: 0"), "{text}");
    assert!(text.contains("claude: 0"), "{text}");
    assert!(text.contains("codex: 0"), "{text}");
    assert!(text.contains("JSON: pandacode list --json"), "{text}");
    assert!(!text.contains('{'), "{text}");

    let output = Command::new(bin())
        .args(["list", "--json", "--cd", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["bamboo"].as_array().unwrap().len(), 0);
    assert_eq!(json["claude"].as_array().unwrap().len(), 0);
    assert_eq!(json["codex"].as_array().unwrap().len(), 0);

    fs::remove_dir_all(root).unwrap();
}

fn write_exe(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn fake_codex_appserver(path: &Path) {
    let py = path.with_extension("py");
    fs::write(
        &py,
        r#"
import json, os, sys, time

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

asked = False
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    mid = msg.get("id")
    if method is None:
        if mid == 999:
            send({"method": "item/completed", "params": {"item": {"type": "agentMessage", "text": "answered:" + json.dumps(msg.get("result"))}}})
            send({"method": "thread/tokenUsage/updated", "params": {"total": {"input_tokens": 11, "output_tokens": 6}}})
            send({"method": "turn/completed", "params": {"turn": {"id": "turn_fake", "status": "completed"}}})
        continue
    if method == "initialize":
        send({"id": mid, "result": {"codexHome": "/tmp"}})
    elif method in ("thread/start", "thread/resume"):
        send({"id": mid, "result": {"thread": {"id": "thread_fake", "path": "/tmp/thread_fake.jsonl"}, "model": "gpt-5.5"}})
    elif method == "turn/start":
        send({"id": mid, "result": {"turn": {"id": "turn_fake"}}})
        if os.environ.get("FAKE_TURN_SLEEP"):
            time.sleep(float(os.environ["FAKE_TURN_SLEEP"]))
        if os.environ.get("FAKE_REQUEST_USER_INPUT") and not asked:
            asked = True
            send({"id": 999, "method": "item/tool/requestUserInput", "params": {"questions": [{"id": "q1", "question": "How should this continue?", "options": [{"label": "keep going"}, {"label": "stop here"}]}]}})
            continue
        try:
            text = msg["params"]["input"][0]["text"]
        except Exception:
            text = ""
        send({"method": "item/completed", "params": {"item": {"type": "agentMessage", "text": "implemented:" + text[:200]}}})
        send({"method": "thread/tokenUsage/updated", "params": {"total": {"input_tokens": 10, "output_tokens": 5}}})
        send({"method": "turn/completed", "params": {"turn": {"id": "turn_fake", "status": "completed"}}})
    elif method == "model/list":
        send({"id": mid, "result": {"data": [{"id": "gpt-5.5", "displayName": "GPT-5.5", "isDefault": True, "supportedReasoningEfforts": ["low", "medium", "high", "xhigh"]}]}})
    elif mid is not None:
        send({"id": mid, "result": {}})
"#,
    )
    .unwrap();
    write_exe(
        path,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${{1:-}}" == "--help" ]]; then
  echo "fake codex help"
  exit 0
fi
if [[ "${{1:-}}" == "app-server" ]]; then
  exec /usr/bin/env python3 "{py}" app-server
fi
echo "unknown fake codex args: $*" >&2
exit 2
"#,
            py = py.display()
        ),
    );
}

fn fake_claude(path: &Path) {
    write_exe(
        path,
        r#"#!/usr/bin/env bash
if [[ "${1:-}" == "--help" ]]; then
  echo "fake claude help --model sonnet --dangerously-skip-permissions"
  exit 0
fi
sleep 60
"#,
    );
}

fn fake_tmux(path: &Path, state: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
STATE={state}
mkdir -p "$STATE/sessions"
cmd="${{1:-}}"
shift || true
target=""
session_arg() {{
  local prev=""
  for arg in "$@"; do
    if [[ "$prev" == "-t" || "$prev" == "-s" ]]; then
      echo "$arg"
      return
    fi
    prev="$arg"
  done
}}
case "$cmd" in
  -V)
    echo "tmux fake 1.0"
    ;;
  has-session)
    target="$(session_arg "$@")"
    [[ -f "$STATE/sessions/$target" ]]
    ;;
	  new-session)
	    target="$(session_arg "$@")"
	    touch "$STATE/sessions/$target"
	    cat > "$STATE/$target.pane" <<'PANE'
Quick safety check: Is this a project you created or one you trust?

❯ 1. Yes, I trust this folder
  2. No, exit

Enter to confirm · Esc to cancel
PANE
	    ;;
  load-buffer)
    target="$(session_arg "$@")"
    cat > "$STATE/$target.buffer"
    ;;
	  paste-buffer)
	    target="$(session_arg "$@")"
	    cat "$STATE/$target.buffer" >> "$STATE/$target.pane"
	    marker="$(grep -o 'PANDACODE_DONE_[0-9_]*' "$STATE/$target.buffer" | tail -n 1 || true)"
	    if [[ -n "$marker" ]] && ! grep -q 'NO_FAKE_COMPLETE' "$STATE/$target.buffer"; then
	      printf '\n● fake response\n%s\n❯\n' "$marker" >> "$STATE/$target.pane"
	    fi
	    ;;
	  send-keys)
	    target="$(session_arg "$@")"
	    if grep -q 'Quick safety check' "$STATE/$target.pane" 2>/dev/null && printf '%s\n' "$@" | grep -q Enter; then
	      cat > "$STATE/$target.pane" <<'PANE'
╭─── Claude Code v2.1.160 ─────────────────────────────────────────────────────╮
╰──────────────────────────────────────────────────────────────────────────────╯

────────────────────────────────────────────────────────────────────────────────
❯ Try "fix lint errors"
────────────────────────────────────────────────────────────────────────────────
PANE
	    elif printf '%s\n' "$@" | grep -q Escape; then
	      printf '\nInterrupted\n❯\n' >> "$STATE/$target.pane"
	    fi
	    ;;
  capture-pane)
    target="$(session_arg "$@")"
    cat "$STATE/$target.pane"
    ;;
	  kill-session)
	    target="$(session_arg "$@")"
	    rm -f "$STATE/sessions/$target"
	    rm -f "$STATE/$target.pane" "$STATE/$target.buffer"
	    ;;
  list-sessions)
    for f in "$STATE"/sessions/*; do
      [[ -e "$f" ]] || exit 0
      basename "$f"
    done
    ;;
  *)
    echo "unknown fake tmux cmd $cmd $*" >&2
    exit 2
    ;;
esac
"#,
        state = state.display()
    );
    write_exe(path, &script);
}

fn fake_openai_compatible_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_thread = Arc::clone(&calls);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let call_index = calls_for_thread.fetch_add(1, Ordering::SeqCst);
            handle_fake_openai_request(stream, call_index);
        }
    });
    format!("http://{addr}")
}

fn fake_openai_ask_then_finish_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_thread = Arc::clone(&calls);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let call_index = calls_for_thread.fetch_add(1, Ordering::SeqCst);
            handle_fake_openai_response(stream, fake_openai_ask_then_finish_response(call_index));
        }
    });
    format!("http://{addr}")
}

fn fake_openai_error_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _request = read_http_request(&mut stream);
            let body = serde_json::json!({
                "error": {
                    "message": "quota exhausted",
                    "type": "rate_limit_error"
                }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{addr}")
}

fn handle_fake_openai_request(stream: TcpStream, call_index: usize) {
    handle_fake_openai_response(stream, fake_openai_response(call_index));
}

fn handle_fake_openai_response(mut stream: TcpStream, body: String) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _request = read_http_request(&mut stream);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        let read = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        data.extend_from_slice(&buf[..read]);
        if request_complete(&data) {
            break;
        }
    }
    data
}

fn request_complete(data: &[u8]) -> bool {
    let Some(header_end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&data[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.eq_ignore_ascii_case("content-length"))
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    data.len() >= header_end + 4 + content_length
}

fn fake_openai_response(call_index: usize) -> String {
    let (tool_name, arguments) = match call_index % 3 {
        0 => (
            "write",
            serde_json::json!({
                "path": "native.txt",
                "content": "ok\n",
                "create_dirs": true
            }),
        ),
        1 => (
            "bash",
            serde_json::json!({
                "cmd": "test -f native.txt && grep -q ok native.txt",
                "timeout_ms": 120000
            }),
        ),
        _ => (
            "finish",
            serde_json::json!({
                "status": "success",
                "summary": "native fake implemented",
                "verification": ["test -f native.txt && grep -q ok native.txt"]
            }),
        ),
    };
    serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": format!("call-{call_index}"),
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": serde_json::to_string(&arguments).unwrap()
                    }
                }]
            }
        }],
        "usage": {
            "prompt_tokens": 100 + call_index as u64,
            "completion_tokens": 20,
            "total_tokens": 120 + call_index as u64,
            "prompt_cache_hit_tokens": 80,
            "prompt_cache_miss_tokens": 20
        }
    })
    .to_string()
}

fn fake_openai_ask_then_finish_response(call_index: usize) -> String {
    let (tool_name, arguments) = if call_index == 0 {
        (
            "ask_user",
            serde_json::json!({
                "question": "Which branch should be updated?",
                "context": "The task needs external branch selection."
            }),
        )
    } else {
        (
            "finish",
            serde_json::json!({
                "status": "success",
                "summary": "continued after user answer",
                "verification": ["answer accepted"]
            }),
        )
    };
    serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": format!("ask-call-{call_index}"),
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": serde_json::to_string(&arguments).unwrap()
                    }
                }]
            }
        }],
        "usage": {
            "prompt_tokens": 42 + call_index as u64,
            "completion_tokens": 7,
            "total_tokens": 49 + call_index as u64
        }
    })
    .to_string()
}

fn command_with_bamboo_env(root: &Path, base_url: &str) -> Command {
    let mut command = Command::new(bin());
    command
        .env("BAMBOO_CONFIG_DIR", root.join(".bamboo-config"))
        .env("BAMBOO_BASE_URL", base_url)
        .env("BAMBOO_API_KEY", "fake-key")
        .env("DEEPSEEK_BASE_URL", base_url)
        .env("DEEPSEEK_API_KEY", "fake-key")
        .env("KIMI_BASE_URL", base_url)
        .env("KIMI_API_KEY", "fake-key")
        .env("XIAOMI_BASE_URL", base_url)
        .env("XIAOMI_API_KEY", "fake-key")
        .env("ZHIPU_BASE_URL", base_url)
        .env("ZHIPU_API_KEY", "fake-key")
        .env("MINIMAX_BASE_URL", base_url)
        .env("MINIMAX_API_KEY", "fake-key")
        .env("QWEN_BASE_URL", base_url)
        .env("QWEN_API_KEY", "fake-key")
        .env("STEPFUN_BASE_URL", base_url)
        .env("STEPFUN_API_KEY", "fake-key");
    command
}

#[test]
fn bamboo_runtime_exec_resume_observe_with_native_fake_provider() {
    let root = temp_root("bamboo");
    let base_url = fake_openai_compatible_server();

    let common = ["--cd", root.to_str().unwrap()];
    let output = command_with_bamboo_env(&root, &base_url)
        .args([
            "bamboo",
            "exec",
            "--task",
            "fix tests",
            "--provider",
            "deepseek",
            "--model",
            "deepseek-v4-pro",
            "--effort",
            "high",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["runtime"], "bamboo");
    assert_eq!(exec["state"], "completed");
    assert_eq!(exec["driver"], "bamboo-native");
    assert!(
        exec["summary"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("native fake implemented")
    );
    assert!(
        exec["record"]["run_id"]
            .as_str()
            .unwrap()
            .starts_with("run-")
    );
    assert_eq!(fs::read_to_string(root.join("native.txt")).unwrap(), "ok\n");
    assert!(exec.get("raw").is_none());

    for args in [
        vec!["bamboo", "status", "--session", "latest"],
        vec!["bamboo", "logs", "--session", "latest", "--json"],
        vec!["bamboo", "artifacts", "--session", "latest"],
        vec![
            "bamboo",
            "model",
            "--session",
            "latest",
            "--provider",
            "deepseek",
            "--model",
            "deepseek-v4-pro",
            "--effort",
            "high",
        ],
        vec![
            "bamboo",
            "resume",
            "--session",
            "latest",
            "--task",
            "continue",
        ],
        vec!["bamboo", "interrupt", "--session", "latest"],
        vec!["bamboo", "stop", "--session", "latest"],
        vec!["bamboo", "list"],
        vec!["bamboo", "models"],
        vec!["bamboo", "doctor"],
    ] {
        let output = command_with_bamboo_env(&root, &base_url)
            .args(args)
            .args(common)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = command_with_bamboo_env(&root, &base_url)
        .args(["bamboo", "logs", "--session", "latest", "--json"])
        .args(common)
        .output()
        .unwrap();
    assert!(output.status.success());
    let logs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(logs.get("capture").is_none());
    assert!(logs["log_tail"].as_str().unwrap().contains("run.started"));

    let output = command_with_bamboo_env(&root, &base_url)
        .args([
            "bamboo",
            "answer",
            "--session",
            "latest",
            "--text",
            "use option A",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not waiting for user input"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bamboo_runtime_ask_user_returns_waiting_and_answer_resumes() {
    let root = temp_root("bamboo-ask");
    let base_url = fake_openai_ask_then_finish_server();
    let common = ["--cd", root.to_str().unwrap()];

    let output = command_with_bamboo_env(&root, &base_url)
        .args([
            "bamboo",
            "exec",
            "--task",
            "ask if branch is missing",
            "--provider",
            "deepseek",
            "--model",
            "deepseek-v4-pro",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["ok"], true);
    assert_eq!(exec["state"], "waiting_for_user");
    assert_eq!(
        exec["pending_user_input"]["question"],
        "Which branch should be updated?"
    );

    let output = command_with_bamboo_env(&root, &base_url)
        .args(["bamboo", "status", "--session", "latest"])
        .args(common)
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["state"], "waiting_for_user");
    assert_eq!(
        status["pending_user_input"]["question"],
        "Which branch should be updated?"
    );

    let output = command_with_bamboo_env(&root, &base_url)
        .args(["bamboo", "answer", "--session", "latest", "--text", "main"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["state"], "completed");
    assert!(
        answer["summary"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("continued after user answer")
    );
    assert!(answer["pending_user_input"].is_null());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn top_level_agent_commands_auto_select_and_resume_latest_runtime() {
    let root = temp_root("agent-top-level");
    let base_url = fake_openai_compatible_server();
    let common = ["--cd", root.to_str().unwrap()];

    let output = command_with_bamboo_env(&root, &base_url)
        .args([
            "run",
            "--task",
            "fix tests",
            "--provider",
            "deepseek",
            "--model",
            "deepseek-v4-pro",
            "--effort",
            "high",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["runtime"], "bamboo");
    assert_eq!(run["state"], "completed");

    let output = command_with_bamboo_env(&root, &base_url)
        .args(["status"])
        .args(common)
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["runtime"], "bamboo");
    assert_eq!(status["state"], "completed");

    let output = command_with_bamboo_env(&root, &base_url)
        .args(["resume", "--task", "continue"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resume: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(resume["runtime"], "bamboo");
    assert_eq!(resume["state"], "completed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn top_level_run_infers_bamboo_provider_from_model() {
    let root = temp_root("agent-model-infers-bamboo");
    let base_url = fake_openai_compatible_server();
    let output = command_with_bamboo_env(&root, &base_url)
        .args([
            "run",
            "--task",
            "fix tests",
            "--model",
            "kimi-k2.6",
            "--effort",
            "high",
            "--cd",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["runtime"], "bamboo");
    assert_eq!(run["summary"]["provider"], "kimi");
    assert_eq!(run["summary"]["model"], "kimi-k2.6");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn top_level_run_infers_codex_from_model() {
    let root = temp_root("agent-model-infers-codex");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    fake_codex_appserver(&codex);

    let output = Command::new(bin())
        .args([
            "run",
            "--task",
            "fix tests",
            "--model",
            "gpt-5.5",
            "--effort",
            "xhigh",
            "--cd",
            root.to_str().unwrap(),
            "--codex-bin",
            codex.to_str().unwrap(),
        ])
        .env_remove("BAMBOO_API_KEY")
        .env_remove("PANDACODE_BAMBOO_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["runtime"], "codex");
    assert_eq!(run["record"]["model"], "gpt-5.5");
    assert_eq!(run["record"]["effort"], "xhigh");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn top_level_agent_commands_resolve_named_session_runtime() {
    let root = temp_root("agent-named-session");
    let base_url = fake_openai_compatible_server();
    let common = ["--cd", root.to_str().unwrap()];

    let output = command_with_bamboo_env(&root, &base_url)
        .args([
            "run",
            "--session",
            "named-bamboo",
            "--task",
            "fix tests",
            "--provider",
            "deepseek",
            "--model",
            "deepseek-v4-pro",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = command_with_bamboo_env(&root, &base_url)
        .args(["status", "--session", "named-bamboo"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["runtime"], "bamboo");
    assert_eq!(status["session"], "named-bamboo");
    assert_eq!(status["state"], "completed");

    let output = command_with_bamboo_env(&root, &base_url)
        .args(["resume", "--session", "named-bamboo", "--task", "continue"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resume: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(resume["runtime"], "bamboo");
    assert_eq!(resume["state"], "completed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn json_flag_returns_structured_errors() {
    let root = temp_root("json-error");
    let output = Command::new(bin())
        .args([
            "run",
            "--task",
            "fix tests",
            "--provider",
            "deepseek",
            "--json",
            "--cd",
            root.to_str().unwrap(),
        ])
        .env("BAMBOO_CONFIG_DIR", root.join(".bamboo-config"))
        .env_remove("BAMBOO_API_KEY")
        .env_remove("OPENCLAUDE_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(error["state"], "failed");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing API key")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn models_json_reports_permission_capabilities() {
    let root = temp_root("models-permissions");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    let claude = bin_dir.join("claude");
    fake_codex_appserver(&codex);
    fake_claude(&claude);

    let output = Command::new(bin())
        .args([
            "models",
            "--json",
            "--cd",
            root.to_str().unwrap(),
            "--codex-bin",
            codex.to_str().unwrap(),
            "--claude-bin",
            claude.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let models: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for runtime in ["codex", "claude", "bamboo"] {
        assert_eq!(
            models[runtime]["capabilities"]["permissions_supported"][0], "max",
            "{runtime}"
        );
        assert_eq!(
            models[runtime]["capabilities"]["permissions_supported"][1], "limited",
            "{runtime}"
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bamboo_blocked_run_emits_single_json_object() {
    let root = temp_root("bamboo-blocked-json");
    let base_url = fake_openai_error_server();
    let output = command_with_bamboo_env(&root, &base_url)
        .args([
            "run",
            "--task",
            "fix tests",
            "--provider",
            "deepseek",
            "--json",
            "--cd",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["state"], "blocked");
    assert!(
        response["summary"]["summary"]
            .as_str()
            .unwrap()
            .contains("quota exhausted")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_runtime_exec_resume_observe_and_stop_with_fake_appserver() {
    let root = temp_root("codex");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    fake_codex_appserver(&codex);

    let common = [
        "--cd",
        root.to_str().unwrap(),
        "--codex-bin",
        codex.to_str().unwrap(),
    ];
    let output = Command::new(bin())
        .args(["codex", "exec", "--task", "fix tests"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["ok"], true);
    assert_eq!(exec["state"], "completed");
    assert_eq!(exec["record"]["thread_id"], "thread_fake");
    assert!(
        exec["summary"]["last_agent_message"]
            .as_str()
            .unwrap()
            .starts_with("implemented:")
    );
    assert!(exec["summary"]["usage"]["total"]["input_tokens"].is_number());

    let output = Command::new(bin())
        .args(["codex", "status", "--session", "latest"])
        .args(common)
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["ok"], true);
    assert_eq!(status["state"], "completed");
    assert!(
        status["summary"]["last_agent_message"]
            .as_str()
            .unwrap()
            .starts_with("implemented:")
    );

    let output = Command::new(bin())
        .args(["codex", "logs", "--session", "latest", "--json"])
        .args(common)
        .output()
        .unwrap();
    assert!(output.status.success());
    let logs: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(logs["ok"], true);
    assert!(logs["log_tail"].as_str().unwrap().contains("turn/completed"));

    for args in [
        vec!["codex", "artifacts", "--session", "latest"],
        vec![
            "codex",
            "model",
            "--session",
            "latest",
            "--model",
            "gpt-5.5",
            "--effort",
            "xhigh",
        ],
    ] {
        let output = Command::new(bin()).args(args).args(common).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = Command::new(bin())
        .args([
            "codex",
            "resume",
            "--session",
            "latest",
            "--task",
            "continue",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resume: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(resume["state"], "completed");
    assert!(
        resume["summary"]["last_agent_message"]
            .as_str()
            .unwrap()
            .contains("continue")
    );

    let output = Command::new(bin())
        .args([
            "codex", "answer", "--session", "latest", "--text", "go", "--wait",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["state"], "completed");
    assert!(
        answer["summary"]["last_agent_message"]
            .as_str()
            .unwrap()
            .contains("go")
    );

    let output = Command::new(bin())
        .args(["codex", "models"])
        .args(common)
        .output()
        .unwrap();
    assert!(output.status.success());
    let models: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(models["ok"], true);
    assert_eq!(models["models"][0]["id"], "gpt-5.5");

    for args in [
        vec!["codex", "interrupt", "--session", "latest"],
        vec!["codex", "stop", "--session", "latest"],
        vec!["codex", "list"],
        vec!["codex", "doctor"],
    ] {
        let output = Command::new(bin()).args(args).args(common).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_status_is_visible_while_start_is_running() {
    let root = temp_root("codex-start-visible");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    fake_codex_appserver(&codex);

    let common = [
        "--cd",
        root.to_str().unwrap(),
        "--codex-bin",
        codex.to_str().unwrap(),
    ];
    let child = Command::new(bin())
        .env("FAKE_TURN_SLEEP", "2")
        .args(["codex", "exec", "--task", "slow start", "--session", "slow"])
        .args(common)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let record_path = root.join(".pandacode/sessions/codex/slow.json");
    let mut running_seen = false;
    for _ in 0..100 {
        if record_path.exists()
            && let Ok(text) = fs::read_to_string(&record_path)
            && let Ok(record) = serde_json::from_str::<serde_json::Value>(&text)
        {
            let status = record["artifacts"]["status"].as_str().unwrap_or("");
            if status == "starting" || status == "running" {
                running_seen = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        running_seen,
        "codex session record should be visible while the turn is still running"
    );

    let output = Command::new(bin())
        .args(["codex", "status", "--session", "slow"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(matches!(
        status["state"].as_str().unwrap(),
        "starting" | "running" | "completed"
    ));

    let output = Command::new(bin())
        .args(["list", "--cd", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let list = String::from_utf8_lossy(&output.stdout);
    assert!(list.contains("codex: 1"), "{list}");

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["state"], "completed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_orphaned_appserver_is_reaped_on_next_run() {
    let root = temp_root("codex-orphan-reap");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    fake_codex_appserver(&codex);
    let common = [
        "--cd",
        root.to_str().unwrap(),
        "--codex-bin",
        codex.to_str().unwrap(),
    ];

    let mut child = Command::new(bin())
        .env("FAKE_TURN_SLEEP", "30")
        .args(["codex", "exec", "--task", "long turn", "--session", "orphan"])
        .args(common)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let record_path = root.join(".pandacode/sessions/codex/orphan.json");
    let mut turn_running = false;
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&record_path)
            && let Ok(record) = serde_json::from_str::<serde_json::Value>(&text)
            && record["artifacts"]["status"] == "running"
        {
            turn_running = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(turn_running, "exec should reach the running state");
    thread::sleep(Duration::from_millis(300));

    let pid_dir = root.join(".pandacode/codex/appserver-pids");
    let appserver_pid = fs::read_dir(&pid_dir)
        .unwrap()
        .flatten()
        .next()
        .and_then(|entry| entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
        .expect("app-server pid file should exist while the turn runs");

    // SIGKILL pandacode so destructors never run; the app-server is orphaned.
    child.kill().unwrap();
    child.wait().unwrap();
    thread::sleep(Duration::from_millis(300));
    let alive = Command::new("/bin/kill")
        .args(["-0", &appserver_pid.to_string()])
        .status()
        .unwrap()
        .success();
    assert!(alive, "fake app-server should still be alive after pandacode dies");

    let output = Command::new(bin())
        .args(["codex", "doctor"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut reaped = false;
    for _ in 0..40 {
        let alive = Command::new("/bin/kill")
            .args(["-0", &appserver_pid.to_string()])
            .status()
            .unwrap()
            .success();
        if !alive {
            reaped = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        reaped,
        "orphaned app-server should be killed by the next pandacode codex command"
    );
    assert!(!pid_dir.join(appserver_pid.to_string()).exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_detached_exec_answers_structurally_in_same_turn() {
    let root = temp_root("codex-detach-answer");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    fake_codex_appserver(&codex);
    let common = [
        "--cd",
        root.to_str().unwrap(),
        "--codex-bin",
        codex.to_str().unwrap(),
    ];

    let output = Command::new(bin())
        .env("FAKE_REQUEST_USER_INPUT", "1")
        .args([
            "codex", "exec", "--detach", "--session", "bg", "--task", "ask then continue",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["detached"], true);
    assert_eq!(exec["state"], "running");

    let record_path = root.join(".pandacode/sessions/codex/bg.json");
    let mut waiting = false;
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&record_path)
            && let Ok(record) = serde_json::from_str::<serde_json::Value>(&text)
            && record["artifacts"]["status"] == "waiting_for_user"
        {
            assert_eq!(record["artifacts"]["pending_questions"][0]["id"], "q1");
            waiting = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(waiting, "detached worker should reach waiting_for_user");

    let output = Command::new(bin())
        .args(["codex", "answer", "--session", "bg", "--choice", "2", "--wait"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["answer_mode"], "structured");
    assert_eq!(answer["state"], "completed");
    let message = answer["summary"]["last_agent_message"].as_str().unwrap();
    assert!(message.starts_with("answered:"), "{message}");
    assert!(message.contains("stop here"), "{message}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_request_user_input_pauses_and_answer_resumes() {
    let root = temp_root("codex-answer-pending");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    fake_codex_appserver(&codex);

    let common = [
        "--cd",
        root.to_str().unwrap(),
        "--codex-bin",
        codex.to_str().unwrap(),
        "--json",
    ];
    let output = Command::new(bin())
        .env("FAKE_REQUEST_USER_INPUT", "1")
        .args([
            "codex",
            "exec",
            "--session",
            "needs-input",
            "--task",
            "ask then continue",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["state"], "waiting_for_user");
    assert_eq!(
        exec["pending_user_input"]["questions"][0]["question"],
        "How should this continue?"
    );

    let output = Command::new(bin())
        .args([
            "codex",
            "answer",
            "--session",
            "needs-input",
            "--choice",
            "2",
            "--wait",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["ok"], true);
    assert_eq!(answer["state"], "completed");
    let message = answer["summary"]["last_agent_message"].as_str().unwrap();
    assert!(message.ends_with("stop here"), "{message}");

    let record =
        fs::read_to_string(root.join(".pandacode/sessions/codex/needs-input.json")).unwrap();
    let record: serde_json::Value = serde_json::from_str(&record).unwrap();
    assert!(record["artifacts"]["pending_questions"].is_null());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wait_fast_fails_on_unknown_session() {
    let root = temp_root("wait-fastfail");
    let output = Command::new(bin())
        .args([
            "wait",
            "--session",
            "never-launched",
            "--timeout-ms",
            "60000",
            "--interval-ms",
            "500",
            "--cd",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "unknown session must fail");
    let wait: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(wait["state"], "missing_session");
    assert_eq!(wait["missing_sessions"][0], "never-launched");
    // Must fail fast (grace window ~10s), not wait the full 60s timeout.
    assert!((wait["elapsed_ms"].as_u64().unwrap()) < 30_000);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wait_flags_missing_reports_and_claude_detach_runs_in_background() {
    let root = temp_root("wait-no-report");
    let bin_dir = root.join("bin");
    let state = root.join("tmux-state");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&state).unwrap();
    let tmux = bin_dir.join("tmux");
    let claude = bin_dir.join("claude");
    fake_tmux(&tmux, &state);
    fake_claude(&claude);
    let common = [
        "--cd",
        root.to_str().unwrap(),
        "--tmux-bin",
        tmux.to_str().unwrap(),
        "--claude-bin",
        claude.to_str().unwrap(),
    ];

    let output = Command::new(bin())
        .args([
            "claude",
            "exec",
            "--detach",
            "--session",
            "lane1",
            "--task",
            "review and write a report",
            "--expect-artifact",
            "result/lane1.md",
        ])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["detached"], true);
    assert_eq!(exec["state"], "running");

    let output = Command::new(bin())
        .args([
            "wait",
            "--session",
            "lane1",
            "--expect-artifact",
            "result/lane1.md",
            "--timeout-ms",
            "30000",
            "--interval-ms",
            "300",
            "--cd",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "wait should fail when the report is missing"
    );
    let wait: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(wait["ok"], false);
    assert_eq!(wait["sessions"]["lane1"]["state"], "no_report");
    assert_eq!(wait["missing_artifacts"][0], "result/lane1.md");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn claude_runtime_exec_resume_observe_and_stop_with_fake_tmux() {
    let root = temp_root("claude");
    let bin_dir = root.join("bin");
    let state = root.join("tmux-state");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&state).unwrap();
    let tmux = bin_dir.join("tmux");
    let claude = bin_dir.join("claude");
    fake_tmux(&tmux, &state);
    fake_claude(&claude);

    let common = [
        "--cd",
        root.to_str().unwrap(),
        "--tmux-bin",
        tmux.to_str().unwrap(),
        "--claude-bin",
        claude.to_str().unwrap(),
    ];
    let output = Command::new(bin())
        .args(["claude", "exec", "--task", "fix tests", "--session", "main"])
        .args(common)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["session"], "main");
    assert_eq!(exec["state"], "completed");
    assert_eq!(exec["wait"]["ok"], true);
    assert!(!state.join("sessions/main").exists());

    let output = Command::new(bin())
        .args(["claude", "status", "--session", "main"])
        .args(common)
        .output()
        .unwrap();
    assert!(output.status.success());
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(status.get("capture").is_none());
    // Top-level state mirrors the recorded outcome (matches codex + wait);
    // the dead tmux session shows up under live_state.
    assert_eq!(status["state"], "completed");
    assert_eq!(status["live_state"], "stopped");

    for args in [
        vec!["claude", "status", "--session", "main"],
        vec!["claude", "artifacts", "--session", "main"],
        vec![
            "claude",
            "model",
            "--session",
            "main",
            "--model",
            "sonnet",
            "--effort",
            "high",
        ],
        vec![
            "claude",
            "resume",
            "--session",
            "main",
            "--task",
            "continue",
        ],
        vec!["claude", "interrupt", "--session", "main"],
        vec!["claude", "stop", "--session", "main"],
        vec!["claude", "list"],
        vec!["claude", "models"],
        vec!["claude", "doctor"],
    ] {
        let output = Command::new(bin())
            .args(args)
            .args(common)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!state.join("sessions/main").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn claude_runtime_timeout_cleans_fake_tmux() {
    let root = temp_root("claude-timeout");
    let bin_dir = root.join("bin");
    let state = root.join("tmux-state");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&state).unwrap();
    let tmux = bin_dir.join("tmux");
    let claude = bin_dir.join("claude");
    fake_tmux(&tmux, &state);
    fake_claude(&claude);

    let output = Command::new(bin())
        .args([
            "claude",
            "exec",
            "--task",
            "NO_FAKE_COMPLETE",
            "--session",
            "timeout-main",
            "--timeout-ms",
            "50",
            "--cd",
            root.to_str().unwrap(),
            "--tmux-bin",
            tmux.to_str().unwrap(),
            "--claude-bin",
            claude.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exec: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(exec["state"], "timeout");
    assert_eq!(exec["ok"], false);
    assert!(!state.join("sessions/timeout-main").exists());

    fs::remove_dir_all(root).unwrap();
}
