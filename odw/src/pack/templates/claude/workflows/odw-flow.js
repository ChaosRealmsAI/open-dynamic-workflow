// Open Dynamic Workflow complex flow starter.
//
// This script is meant to be called by an Agent or CLI, not by a slash command.
// Real runs should pass input.prompts.<slot>; mock runs may use suggested slots.

const schemaDescriptions = {
  taskPlan: "Final response describes the decomposed workflow plan: task list, join policy, quality gate policy, risks, and questions.",
  codexResult: "Final response summarizes one Codex executor node: run id, status, changed files, verification evidence, risks, and adapter metadata.",
  taskJoin: "Final response joins implementation evidence into reviewable task items, failures, and review targets.",
  verifier: "Final response records evidence-backed accepted claims, rejected claims, and claims needing more evidence.",
  qualityGate: "Final response decides whether the joined result passes, needs bounded rework, or fails, including concrete rework tasks when needed.",
  synthesis: "Final response is the workflow report for the caller, summarizing outcome, details, risks, and next actions.",
  errorFeedback: "Final response classifies a failed node into retryability, user-facing message, next action, and failure evidence."
};

export const meta = {
  name: "odw-flow",
  description: "Dynamic workflow with decomposition, fan-out, join, verification, parallel review, quality gate, and bounded rework.",
  phases: [
    { title: "Plan", detail: "Decompose the request into parallel task nodes" },
    { title: "Implement", detail: "Fan out independent PandaCode Codex executor runs" },
    { title: "Join", detail: "Collect task results into one Claude evidence object" },
    { title: "Verify", detail: "Verify the joined result" },
    { title: "Review", detail: "Fan out independent review nodes" },
    { title: "Quality", detail: "Decide pass, fail, or rework" },
    { title: "Rework", detail: "Loop bounded rework tasks back into the join" },
    { title: "Synthesize", detail: "Return the final report" }
  ],
  promptSlots: [
    "plan_decompose",
    "task_start",
    "task_execute",
    "task_read",
    "join_results",
    "verify_joined",
    "review_target",
    "quality_gate",
    "synthesize",
    "failure_feedback"
  ],
  schemas: [
    ".odw/schemas/task-plan.schema.json",
    ".odw/schemas/codex-result.schema.json",
    ".odw/schemas/task-join.schema.json",
    ".odw/schemas/verifier.schema.json",
    ".odw/schemas/quality-gate.schema.json",
    ".odw/schemas/synthesis.schema.json",
    ".odw/schemas/error-feedback.schema.json"
  ]
};

const input = args;
const request = workflowRequest(input);
const taskLimit = Math.min(Number(input?.maxParallelTasks ?? 16), 16);

phase("Plan", "Decompose the request into parallel task nodes");
const plan = await agent(
  promptSlot("plan_decompose", {
    input: request,
    required_schema: ".odw/schemas/task-plan.schema.json",
    max_parallel_tasks: taskLimit,
    note: "Return tasks that can be dispatched independently. Downstream implementation nodes each run PandaCode Codex."
  }, `
You are the workflow planning node.

Context:
{{context}}

Return only JSON matching .odw/schemas/task-plan.schema.json.
`),
  {
    id: "plan",
    label: "plan and decompose",
    phase: "Plan",
    agentType: "odw-orchestrator",
    runtime: "claude",
    schema: ".odw/schemas/task-plan.schema.json",
    schemaDescription: schemaDescriptions.taskPlan,
    retry: { maxAttempts: 2 }
  }
);

if (isFailure(plan)) {
  phase("Feedback", "Classify plan failure");
  return feedback("plan failed", { input: request, plan });
}
if (!Array.isArray(plan.tasks) || plan.tasks.length === 0) {
  phase("Feedback", "Plan returned no tasks");
  return feedback("plan returned no tasks", { input: request, plan });
}

const initialTasks = plan.tasks.slice(0, taskLimit);
checkpoint("after_plan", { task_count: initialTasks.length, max_parallel: taskLimit });

phase("Implement", `Fan out ${initialTasks.length} Codex executor nodes`);
let implementations = await runTaskFanout(initialTasks, {
  phaseName: "Implement",
  labelPrefix: "task",
  iteration: 0,
  prior: null
});
checkpoint("after_implementation", { completed_tasks: implementations.length });

let joined = null;
let verified = null;
let reviews = [];
let quality = null;
const maxReworkIterations = Math.min(Number(plan?.quality?.max_rework_iterations ?? 1), 2);

for (let iteration = 0; iteration <= maxReworkIterations; iteration += 1) {
  phase("Join", `Join implementation evidence, pass ${iteration + 1}`);
  joined = await agent(
    promptSlot("join_results", {
      input: request,
      plan: compactPlan(plan),
      implementations: compactImplementations(implementations),
      iteration,
      required_schema: ".odw/schemas/task-join.schema.json"
    }, `
You are the workflow join node.

Context:
{{context}}

Return only JSON matching .odw/schemas/task-join.schema.json.
`),
    {
      id: `join-${iteration}`,
      label: `join results ${iteration}`,
      phase: "Join",
      agentType: "odw-synthesizer",
      runtime: "claude",
      schema: ".odw/schemas/task-join.schema.json",
      schemaDescription: schemaDescriptions.taskJoin,
      retry: { maxAttempts: 2 }
    }
  );
  if (isFailure(joined)) {
    phase("Feedback", "Classify join failure");
    return feedback("join failed", { input: request, plan: compactPlan(plan), implementations: compactImplementations(implementations), joined, iteration });
  }

  phase("Verify", `Verify joined evidence, pass ${iteration + 1}`);
  verified = await agent(
    promptSlot("verify_joined", {
      input: request,
      plan: compactPlan(plan),
      joined,
      iteration,
      required_schema: ".odw/schemas/verifier.schema.json"
    }, `
You are the workflow verification node.

Context:
{{context}}

Return only JSON matching .odw/schemas/verifier.schema.json.
`),
    {
      id: `verify-${iteration}`,
      label: `verify joined ${iteration}`,
      phase: "Verify",
      agentType: "odw-verifier",
      runtime: "claude",
      schema: ".odw/schemas/verifier.schema.json",
      schemaDescription: schemaDescriptions.verifier,
      retry: { maxAttempts: 2 }
    }
  );
  if (isFailure(verified)) {
    phase("Feedback", "Classify verifier failure");
    return feedback("verify failed", { input: request, plan: compactPlan(plan), implementations: compactImplementations(implementations), joined, verified, iteration });
  }

  const targets = reviewTargets(joined, plan).slice(0, taskLimit);
  phase("Review", `Fan out ${targets.length} independent review nodes`);
  reviews = await fanout(
    targets,
    (target, index) => agent(
      promptSlot("review_target", {
        input: request,
        plan: compactPlan(plan),
        joined,
        verified,
        target,
        index,
        iteration,
        required_schema: ".odw/schemas/verifier.schema.json"
      }, `
You are one parallel workflow review node.

Context:
{{context}}

Return only JSON matching .odw/schemas/verifier.schema.json.
`),
      {
        id: `review-${iteration}-${safeId(target.id ?? index)}`,
        label: `review ${target.title ?? target.id ?? index}`,
        phase: "Review",
        agentType: "odw-verifier",
        runtime: "claude",
        schema: ".odw/schemas/verifier.schema.json",
        schemaDescription: schemaDescriptions.verifier,
        retry: { maxAttempts: 2 }
      }
    ),
    { label: `review fanout ${iteration}`, max: Math.max(1, Math.min(targets.length, taskLimit)) }
  );

  const blockingReviewIssues = reviewBlockingIssues([verified, ...reviews]);
  phase("Quality", `Quality gate, pass ${iteration + 1}`);
  quality = await agent(
    promptSlot("quality_gate", {
      input: request,
      plan: compactPlan(plan),
      joined,
      verified,
      reviews: compactReviews(reviews),
      blocking_review_issues: blockingReviewIssues,
      iteration,
      max_rework_iterations: maxReworkIterations,
      required_schema: ".odw/schemas/quality-gate.schema.json",
      instruction: blockingReviewIssues.length > 0
        ? "Hard rule: verdict must not be pass while blocking_review_issues remain unresolved by fresh evidence. Return rework if another iteration is available; otherwise fail."
        : "Return verdict pass, rework, or fail. For rework, include rework_tasks shaped like task-plan tasks."
    }, `
You are the workflow quality gate.

Context:
{{context}}

If blocking_review_issues is non-empty, do not return pass unless the context
contains direct evidence that every listed issue is already fixed. Otherwise
return rework with concrete rework_tasks, or fail when no rework remains.

Return only JSON matching .odw/schemas/quality-gate.schema.json.
`),
    {
      id: `quality-${iteration}`,
      label: `quality gate ${iteration}`,
      phase: "Quality",
      agentType: "odw-verifier",
      runtime: "claude",
      schema: ".odw/schemas/quality-gate.schema.json",
      schemaDescription: schemaDescriptions.qualityGate,
      retry: { maxAttempts: 2 }
    }
  );
  if (isFailure(quality)) {
    phase("Feedback", "Classify quality gate failure");
    return feedback("quality gate failed", { input: request, plan: compactPlan(plan), implementations: compactImplementations(implementations), joined, verified, reviews: compactReviews(reviews), quality, iteration });
  }
  quality = enforceQualityGate(quality, blockingReviewIssues, iteration, maxReworkIterations);

  checkpoint(`after_quality_${iteration}`, {
    iteration,
    verdict: quality.verdict,
    score: quality.score,
    rework_tasks: Array.isArray(quality.rework_tasks) ? quality.rework_tasks.length : 0
  });

  if (quality.verdict === "pass") {
    break;
  }

  if (
    quality.verdict === "rework"
    && iteration < maxReworkIterations
    && Array.isArray(quality.rework_tasks)
    && quality.rework_tasks.length > 0
  ) {
    const reworkTasks = quality.rework_tasks.slice(0, taskLimit);
    phase("Rework", `Fan out ${reworkTasks.length} bounded rework tasks`);
    const reworkResults = await runTaskFanout(reworkTasks, {
      phaseName: "Rework",
      labelPrefix: `rework-${iteration}`,
      iteration: iteration + 1,
      prior: { joined, verified, reviews: compactReviews(reviews), quality }
    });
    implementations = implementations.concat(reworkResults);
    continue;
  }

  phase("Feedback", "Quality gate did not pass");
  return feedback("quality gate did not pass", { input: request, plan: compactPlan(plan), implementations: compactImplementations(implementations), joined, verified, reviews: compactReviews(reviews), quality, iteration });
}

phase("Synthesize", "Return final workflow report");
return agent(
  promptSlot("synthesize", {
    input: request,
    plan: compactPlan(plan),
    implementations: compactImplementations(implementations),
    joined,
    verified,
    reviews: compactReviews(reviews),
    quality,
    required_schema: ".odw/schemas/synthesis.schema.json"
  }, `
You are the workflow final synthesis node.

Context:
{{context}}

Return only JSON matching .odw/schemas/synthesis.schema.json.
`),
  {
    id: "synthesize",
    label: "flow report",
    phase: "Synthesize",
    agentType: "odw-synthesizer",
    runtime: "claude",
    schema: ".odw/schemas/synthesis.schema.json",
    schemaDescription: schemaDescriptions.synthesis,
    retry: { maxAttempts: 2 }
  }
);

async function runTaskFanout(tasks, options) {
  const selected = Array.isArray(tasks) && tasks.length > 0
    ? tasks.slice(0, taskLimit)
    : [singleFallbackTask()];
  const pending = selected.map((task, index) => ({ task, index, id: safeId(task.id ?? `${options.labelPrefix}-${index}`) }));
  const selectedIds = new Set(pending.map((entry) => entry.id));
  const completed = new Set();
  const results = [];
  let batch = 0;

  while (pending.length > 0) {
    const ready = pending.filter((entry) => taskDependenciesSatisfied(entry.task, selectedIds, completed));
    if (ready.length === 0) {
      return results.concat(pending.map((entry) => failedTask(
        entry.task,
        {
          ok: false,
          status: "failed",
          error: {
            category: "workflow_dependency_cycle",
            message: `Task ${entry.id} could not run because dependencies were not satisfied.`,
            retryable: false
          }
        },
        "dependency cycle or missing dependency"
      )));
    }

    const batchResults = await fanout(
      ready,
      (entry) => runPandaCodeTask(entry.task, entry.index, options),
      { label: `${options.labelPrefix} fanout batch ${batch}`, max: Math.max(1, Math.min(ready.length, taskLimit)) }
    );
    results.push(...batchResults);
    for (const entry of ready) {
      const index = pending.findIndex((candidate) => candidate.id === entry.id);
      if (index >= 0) {
        pending.splice(index, 1);
      }
      const result = batchResults[ready.findIndex((candidate) => candidate.id === entry.id)];
      if (!isFailure(result)) {
        completed.add(entry.id);
      }
    }
    batch += 1;
  }

  return results;
}

function taskDependenciesSatisfied(task, selectedIds, completed) {
  const deps = Array.isArray(task?.depends_on) ? task.depends_on : [];
  return deps.every((dep) => {
    const id = safeId(dep);
    return !selectedIds.has(id) || completed.has(id);
  });
}

async function runPandaCodeTask(task, index, options) {
  const taskId = safeId(task.id ?? `${options.labelPrefix}-${index}`);
  const context = {
    input: request,
    plan: compactPlan(plan),
    task,
    index,
    phase: options.phaseName,
    iteration: options.iteration,
    prior: compactPrior(options.prior),
    runtime: "codex",
    required_schema: ".odw/schemas/codex-result.schema.json"
  };

  const result = await agent(
    promptSlot("task_start", context, `
You are a PandaCode Codex executor node.

Context:
{{context}}

Execute this one coding task through the Codex runtime. Return only JSON matching
.odw/schemas/codex-result.schema.json.
`),
    {
      id: `${options.labelPrefix}-${taskId}-exec`,
      label: `${options.labelPrefix} exec ${taskId}`,
      phase: options.phaseName,
      agentType: "odw-codex-coder",
      runtime: "codex",
      action: "exec",
      schema: ".odw/schemas/codex-result.schema.json",
      schemaDescription: schemaDescriptions.codexResult,
      retry: { maxAttempts: 2 }
    }
  );
  if (isFailure(result)) {
    return failedTask(task, result, "panda codex exec failed");
  }

  return {
    ok: true,
    task_id: task.id ?? taskId,
    task,
    run_id: codexRunId(result),
    started: result,
    executed: result,
    read: result
  };
}

function reviewTargets(joined, plan) {
  if (Array.isArray(joined?.review_targets) && joined.review_targets.length > 0) {
    return joined.review_targets;
  }
  return (plan.tasks ?? []).map((task) => ({
    id: task.id,
    title: task.title,
    evidence: { task }
  }));
}

function failedTask(task, result, reason, runId = null, started = null) {
  return {
    ok: false,
    task_id: task.id ?? "unknown",
    task,
    run_id: runId,
    started: compactCodexResult(started),
    result: compactCodexResult(result),
    error: { category: "workflow_agent_failed", message: reason }
  };
}

function singleFallbackTask() {
  return {
    id: "single",
    title: "Single fallback task",
    prompt: "The planner returned no tasks; handle the original input as one task.",
    agentType: "odw-codex-coder",
    depends_on: [],
    verification: []
  };
}

function feedback(kind, context) {
  return agent(
    promptSlot("failure_feedback", {
      kind,
      context,
      required_schema: ".odw/schemas/error-feedback.schema.json"
    }, `
You are the workflow failure feedback node.

Context:
{{context}}

Return only JSON matching .odw/schemas/error-feedback.schema.json.
`),
    {
      id: `feedback-${safeId(kind)}`,
      label: `feedback ${kind}`,
      phase: "Feedback",
      agentType: "odw-failure-analyst",
      runtime: "claude",
      schema: ".odw/schemas/error-feedback.schema.json",
      schemaDescription: schemaDescriptions.errorFeedback,
      retry: { maxAttempts: 1 }
    }
  );
}

function workflowRequest(input) {
  if (!input || typeof input !== "object") {
    return input;
  }
  const {
    prompts,
    promptSlots,
    ...rest
  } = input;
  return rest;
}

function compactPlan(plan) {
  return {
    status: plan?.status,
    summary: plan?.summary,
    tasks: Array.isArray(plan?.tasks)
      ? plan.tasks.map((task) => ({
        id: task.id,
        title: task.title,
        agentType: task.agentType,
        depends_on: task.depends_on,
        files: task.files,
        verification: task.verification,
        risk: task.risk
      }))
      : [],
    join: plan?.join,
    quality: plan?.quality,
    risks: plan?.risks,
    questions: plan?.questions
  };
}

function compactImplementations(implementations) {
  return Array.isArray(implementations)
    ? implementations.map(compactImplementation)
    : [];
}

function compactImplementation(result) {
  return {
    ok: result?.ok,
    task_id: result?.task_id,
    title: result?.task?.title,
    files: result?.task?.files,
    run_id: result?.run_id,
    started: compactCodexResult(result?.started),
    executed: compactCodexResult(result?.executed),
    read: compactCodexResult(result?.read)
  };
}

function compactCodexResult(result) {
  if (!result || typeof result !== "object") {
    return result ?? null;
  }
  const codex = result.codex && typeof result.codex === "object" ? result.codex : {};
  return {
    ok: result.ok,
    status: result.status ?? codex.status,
    run_id: result.run_id ?? result.runId ?? codex.run_id ?? codex.runId,
    thread_id: result.thread_id ?? result.threadId ?? codex.thread_id ?? codex.threadId,
    changed_files: compactStringList(result.changed_files, 20, 160),
    verification: compactVerification(result.verification),
    risks: compactStringList(result.risks, 8, 500),
    questions: Array.isArray(result.questions ?? codex.questions)
      ? (result.questions ?? codex.questions).slice(0, 3)
      : result.questions ?? codex.questions,
    needs_input: result.needs_input ?? result.needsInput ?? codex.needs_input ?? codex.needsInput,
    last_agent_message: compactText(result.last_agent_message ?? codex.last_agent_message ?? codex.lastAgentMessage),
    error: compactError(result.error),
    adapter: {
      backend: result.adapter?.backend ?? result.backend ?? codex.backend,
      session_socket: result.session_socket ?? result.adapter?.session_socket ?? codex.session_socket,
      log_dir: result.log_dir ?? result.adapter?.log_dir ?? codex.log_dir,
      stdout_tail: compactText(result.adapter?.stdout_tail ?? result.stdout_tail, 1200, "tail"),
      stderr_tail: compactText(result.adapter?.stderr_tail ?? result.stderr_tail, 1200, "tail")
    }
  };
}

function compactVerification(verification) {
  return Array.isArray(verification)
    ? verification.slice(0, 8).map((item) => ({
      command: compactText(item?.command, 240),
      status: item?.status,
      output_tail: compactText(item?.output_tail, 700, "tail")
    }))
    : verification;
}

function compactStringList(items, maxItems, maxChars) {
  return Array.isArray(items)
    ? items.slice(0, maxItems).map((item) => compactText(item, maxChars))
    : items;
}

function compactError(error) {
  if (!error || typeof error !== "object") {
    return error ?? null;
  }
  return {
    category: error.category,
    message: compactText(error.message, 700),
    retryable: error.retryable,
    next_action: compactText(error.next_action ?? error.nextAction, 700)
  };
}

function compactReviews(reviews) {
  return Array.isArray(reviews)
    ? reviews.map((review) => ({
      verdict: review?.verdict,
      status: review?.status,
      summary: compactText(review?.summary),
      accepted: compactReviewItems(review?.accepted),
      rejected: compactReviewItems(review?.rejected),
      needs_more_evidence: compactReviewItems(review?.needs_more_evidence ?? review?.needsMoreEvidence),
      reasons: review?.reasons,
      risks: review?.risks,
      evidence: review?.evidence,
      error: review?.error
    }))
    : [];
}

function compactReviewItems(items) {
  return Array.isArray(items)
    ? items.slice(0, 8).map((item) => {
      if (!item || typeof item !== "object") {
        return compactText(item, 700);
      }
      return {
        claim: compactText(item.claim ?? item.title ?? item.description, 700),
        evidence: compactText(item.evidence, 900),
        reason: compactText(item.reason, 900),
        required_change: compactText(item.required_change ?? item.requiredChange, 900)
      };
    })
    : [];
}

function reviewBlockingIssues(results) {
  const issues = [];
  for (const [sourceIndex, review] of (Array.isArray(results) ? results : []).entries()) {
    if (!review || typeof review !== "object") {
      continue;
    }
    const source = sourceIndex === 0 ? "verify" : `review-${sourceIndex - 1}`;
    if (isFailure(review)) {
      issues.push({
        source,
        severity: "high",
        claim: "Review node failed or returned an error.",
        evidence: compactText(review?.error?.message ?? review?.error ?? review?.status, 900),
        required_change: "Resolve the review failure before accepting the workflow result."
      });
    }
    for (const item of arrayItems(review.rejected)) {
      issues.push(reviewIssueFromItem(item, source, "high", "Resolve the rejected claim before accepting the workflow result."));
    }
    for (const item of arrayItems(review.needs_more_evidence ?? review.needsMoreEvidence)) {
      issues.push(reviewIssueFromItem(item, source, "medium", "Add evidence or verification before accepting the workflow result."));
    }
  }
  return issues.slice(0, 12);
}

function reviewIssueFromItem(item, source, severity, fallbackRequiredChange) {
  if (!item || typeof item !== "object") {
    return {
      source,
      severity,
      claim: compactText(item, 700),
      evidence: "",
      required_change: fallbackRequiredChange
    };
  }
  return {
    source,
    severity,
    claim: compactText(item.claim ?? item.title ?? item.description ?? item.reason, 700),
    evidence: compactText(item.evidence ?? item.reason, 900),
    required_change: compactText(item.required_change ?? item.requiredChange ?? item.reason ?? fallbackRequiredChange, 900)
  };
}

function enforceQualityGate(quality, blockingIssues, iteration, maxReworkIterations) {
  if (!Array.isArray(blockingIssues) || blockingIssues.length === 0 || quality?.verdict !== "pass") {
    return quality;
  }
  const issues = mergeQualityIssues(quality?.issues, blockingIssues);
  const canRework = iteration < maxReworkIterations;
  if (!canRework) {
    log(`quality gate forced fail: ${blockingIssues.length} unresolved review issue(s)`);
    return {
      ...quality,
      verdict: "fail",
      score: Math.min(Number(quality?.score ?? 0), 0.99),
      issues,
      rework_tasks: [],
      next_action: "fail_due_to_unresolved_review_issues"
    };
  }
  const existingRework = Array.isArray(quality?.rework_tasks) ? quality.rework_tasks : [];
  log(`quality gate forced rework: ${blockingIssues.length} unresolved review issue(s)`);
  return {
    ...quality,
    verdict: "rework",
    score: Math.min(Number(quality?.score ?? 0), 0.79),
    issues,
    rework_tasks: existingRework.length > 0 ? existingRework : reworkTasksFromIssues(blockingIssues),
    next_action: "rework_unresolved_review_issues"
  };
}

function mergeQualityIssues(existing, blockingIssues) {
  const issues = Array.isArray(existing) ? existing.slice() : [];
  for (const issue of blockingIssues) {
    issues.push({
      severity: issue.severity ?? "high",
      claim: issue.claim ?? "Blocking review issue",
      evidence: issue.evidence ?? issue.source ?? "",
      required_change: issue.required_change ?? "Resolve before accepting the workflow result."
    });
  }
  return issues;
}

function reworkTasksFromIssues(blockingIssues) {
  return blockingIssues.slice(0, Math.max(1, Math.min(taskLimit, 4))).map((issue, index) => ({
    id: `resolve-review-issue-${index + 1}`,
    title: `Resolve review issue ${index + 1}`,
    prompt: [
      "Fix the workflow output so this blocking review issue is resolved.",
      `Source: ${issue.source ?? "review"}`,
      `Claim: ${issue.claim ?? "Blocking review issue"}`,
      `Evidence: ${issue.evidence ?? ""}`,
      `Required change: ${issue.required_change ?? "Resolve before accepting."}`,
      "After changing files, run the narrow verification needed to prove the fix."
    ].join("\n"),
    agentType: "odw-codex-coder",
    depends_on: [],
    files: [],
    verification: ["Run the narrow validation commands that prove this review issue is fixed."]
  }));
}

function arrayItems(value) {
  return Array.isArray(value) ? value : [];
}

function compactPrior(prior) {
  if (!prior || typeof prior !== "object") {
    return prior ?? null;
  }
  return {
    joined: prior.joined,
    verified: prior.verified,
    reviews: compactReviews(prior.reviews),
    quality: prior.quality
  };
}

function compactText(value, limit = 2000, mode = "head") {
  const text = String(value ?? "");
  if (text.length <= limit) {
    return text;
  }
  return mode === "tail"
    ? `[truncated]\n${text.slice(-limit)}`
    : `${text.slice(0, limit)}\n[truncated]`;
}

function codexRunId(result) {
  return result?.run_id
    ?? result?.runId
    ?? result?.codex?.run_id
    ?? result?.codex?.runId
    ?? result?.session?.run_id
    ?? null;
}

function isFailure(result) {
  // null/undefined means the node threw or was dropped (parallel/pipeline map a
  // thrown thunk to null) — treat it as a failure so dependents don't proceed on
  // a missing input. A plain string is a successful no-schema text result.
  if (result === null || result === undefined) {
    return true;
  }
  return result?.ok === false
    || result?.status === "failed"
    || result?.status === "stopped"
    || Boolean(result?.error);
}

function safeId(value) {
  return String(value).replace(/[^a-zA-Z0-9_-]+/g, "-").replace(/^-+|-+$/g, "") || "node";
}
