#!/usr/bin/env node

const fs = require("node:fs");

const args = process.argv.slice(2);
const runtime = args[0] || "unknown";
const action = args[1] || "unknown";

function arg(name, fallback = "") {
  const index = args.indexOf(name);
  if (index < 0 || index + 1 >= args.length) {
    return fallback;
  }
  return args[index + 1];
}

const session = arg("--session", `${runtime}-fake-session`);
const taskFile = arg("--task-file");
const task = taskFile ? fs.readFileSync(taskFile, "utf8") : arg("--task", "");

if (process.env.ODW_FAKE_PANDACODE_LOG) {
  fs.appendFileSync(
    process.env.ODW_FAKE_PANDACODE_LOG,
    JSON.stringify({ runtime, action, session, task_file: taskFile, preview: task.slice(0, 160) }) + "\n"
  );
}

function base(value) {
  return {
    ok: true,
    backend: "pandacode",
    runtime,
    action,
    session,
    state: "completed",
    ...value
  };
}

function print(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

if (task.includes("task-plan.schema.json")) {
  print(base({
    status: "planned",
    summary: "fake PandaCode workflow plan",
    tasks: [
      {
        id: "html-shell",
        title: "Create HTML shell",
        prompt: "Create the HTML structure for the smoke app.",
        agentType: "odw-codex-coder",
        depends_on: [],
        files: ["smoke.html"],
        verification: ["inspect smoke.html"]
      },
      {
        id: "html-style",
        title: "Create HTML styling",
        prompt: "Create the CSS styling for the smoke app.",
        agentType: "odw-codex-coder",
        depends_on: [],
        files: ["smoke.html"],
        verification: ["inspect smoke.html"]
      }
    ],
    join: { strategy: "all", expected_outputs: ["implementation result", "verification evidence"] },
    quality: { max_rework_iterations: 1, acceptance: ["all tasks completed", "reviews accepted"] },
    questions: [],
    risks: []
  }));
} else if (task.includes("codex-result.schema.json")) {
  print(base({
    run_id: session,
    status: "completed",
    changed_files: ["smoke.html"],
    verification: [{ command: "fake-pandacode verify", status: "passed", output_tail: "ok" }],
    risks: [],
    adapter: { backend: "pandacode", runtime },
    error: null
  }));
} else if (task.includes("task-join.schema.json")) {
  print(base({
    status: "joined",
    summary: "fake joined PandaCode evidence",
    items: [
      { task_id: "html-shell", status: "completed", run_id: "html-shell", changed_files: ["smoke.html"], verification: [], evidence: "shell done" },
      { task_id: "html-style", status: "completed", run_id: "html-style", changed_files: ["smoke.html"], verification: [], evidence: "style done" }
    ],
    failed: [],
    review_targets: [
      { id: "html-shell", title: "Review HTML shell", evidence: "shell done" },
      { id: "html-style", title: "Review HTML styling", evidence: "style done" }
    ]
  }));
} else if (task.includes("quality-gate.schema.json")) {
  print(base({
    verdict: "pass",
    score: 1,
    accepted: ["fake quality gate passed"],
    issues: [],
    rework_tasks: [],
    next_action: "synthesize"
  }));
} else if (
  task.includes("verifier.schema.json")
  && process.env.ODW_FAKE_PANDACODE_REJECT_FIRST_REVIEW === "1"
  && task.includes("parallel workflow review node")
  && /"iteration":\s*0/.test(task)
) {
  print(base({
    accepted: [],
    rejected: [{
      claim: "fake first-pass review rejection",
      evidence: "fake reviewer found a blocking issue",
      reason: "quality gate must not pass while review has rejected claims",
      required_change: "force rework before final acceptance"
    }],
    needs_more_evidence: []
  }));
} else if (task.includes("verifier.schema.json")) {
  print(base({
    accepted: ["fake verifier accepted evidence"],
    rejected: [],
    needs_more_evidence: []
  }));
} else if (task.includes("synthesis.schema.json")) {
  print(base({
    summary: "fake PandaCode workflow synthesis",
    details: [{ runtimes: ["codex"], note: "PandaCode Codex runtime was exercised by the flow" }],
    risks: [],
    next_actions: []
  }));
} else if (task.includes("error-feedback.schema.json")) {
  print({
    ok: false,
    origin: { phase: "fake", agent: "fake-pandacode", backend: "pandacode" },
    error: { category: "workflow_agent_failed", message: "fake feedback path" },
    feedback: { retryable: false, user_message: "fake feedback", next_action: "none" }
  });
} else {
  print(base({ summary: { last_agent_message: "{}" } }));
}
