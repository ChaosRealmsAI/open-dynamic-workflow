// Open Dynamic Workflow ship starter.
//
// Runs directly with `odw exec`. Scoped implementation through a single-shot
// pandacode Codex executor node, with discovery, testing, and verification.

const schemaDescriptions = {
  research: "Final response maps the minimal implementation surface for downstream planning.",
  codexResult: "Final response is Codex run and implementation evidence for downstream testing and verification.",
  testResult: "Final response records scoped verification commands, statuses, and output tails.",
  verifier: "Final response accepts, rejects, or asks for more evidence on implementation claims.",
  synthesis: "Final response is the verified ship report for the caller.",
  errorFeedback: "Final response classifies a node failure with retryability and next action."
};

export const meta = {
  name: "odw-ship",
  description: "Scoped implementation through a single-shot pandacode Codex executor with verification.",
  phases: [
    { title: "Discover", detail: "Find the minimal change surface" },
    { title: "Implement", detail: "Implement the change via Codex (single-shot)" },
    { title: "Test", detail: "Run scoped verification commands" },
    { title: "Verify", detail: "Check the change against the goal" },
    { title: "Synthesize", detail: "Return the final ship report" }
  ],
  agents: [
    "odw-researcher",
    "odw-codex-coder",
    "odw-test-runner",
    "odw-failure-analyst",
    "odw-verifier",
    "odw-synthesizer"
  ],
  schemas: [
    ".odw/schemas/research.schema.json",
    ".odw/schemas/codex-result.schema.json",
    ".odw/schemas/test-result.schema.json",
    ".odw/schemas/error-feedback.schema.json",
    ".odw/schemas/verifier.schema.json",
    ".odw/schemas/synthesis.schema.json"
  ],
  promptSlots: [
    "discover_surface",
    "codex_implement",
    "test_commands",
    "verify_change",
    "synthesize_report",
    "failure_feedback"
  ]
};

const input = args;

phase("Discover", "Find the minimal change surface");
const surfacePrompt = promptSlot("discover_surface", {
  goal: input,
  required_schema: ".odw/schemas/research.schema.json"
}, `
Role:
Read-only implementation surface mapper.

Input goal:
{{context}}

Task:
Find the smallest repository surface likely needed for this implementation.
Identify files, modules, tests, generated artifacts, and forbidden zones. The
next node will implement against this result, so be specific and conservative.

Constraints:
- Do not edit files.
- Prefer minimal scope over broad exploration.
- Include exact file paths where possible.
- Call out files or areas that must not be touched.
- Return enough evidence to implement without guessing.

Output schema:
.odw/schemas/research.schema.json

Done criteria:
The implementation node has enough file/module boundaries to act without guessing.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
const surface = await agent(surfacePrompt, {
  label: "change surface",
  phase: "Discover",
  agentType: "odw-researcher",
  schema: ".odw/schemas/research.schema.json",
  schemaDescription: schemaDescriptions.research,
  retry: { maxAttempts: 2 }
});

phase("Implement", "Implement the change via Codex (single-shot)");
const implementationPrompt = promptSlot("codex_implement", {
  goal: input,
  surface,
  required_schema: ".odw/schemas/codex-result.schema.json"
}, `
Role:
Codex implementation worker.

Input:
{{context}}

Task:
Implement the requested change in one pass, using the discovered surface as the
source of truth: edit the necessary files, then run the verification the change
needs. Return structured implementation evidence.

Constraints:
- Stay within the discovered surface and respect forbidden zones.
- Do not broaden scope unless the change is otherwise impossible.
- Preserve structured evidence: changed files, verification commands, output
  tails, risks, and next action.

Output schema:
.odw/schemas/codex-result.schema.json

Done criteria:
The change is implemented and the result carries changed files and verification
evidence for downstream test and verifier nodes.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
const codexResult = await agent(implementationPrompt, {
  label: "codex implement",
  phase: "Implement",
  agentType: "odw-codex-coder",
  runtime: "codex",
  sandbox: "danger-full-access",
  approvalPolicy: "never",
  schema: ".odw/schemas/codex-result.schema.json",
  schemaDescription: schemaDescriptions.codexResult,
  retry: { maxAttempts: 2 }
});

if (isFailure(codexResult)) {
  phase("Feedback", "Classify Codex implementation failure");
  return agent(failureFeedbackPrompt("Codex implementation failure", { goal: input, surface, codexResult }), {
    label: "codex implementation failure feedback",
    phase: "Feedback",
    agentType: "odw-failure-analyst",
    schema: ".odw/schemas/error-feedback.schema.json",
    schemaDescription: schemaDescriptions.errorFeedback,
    retry: { maxAttempts: 1 }
  });
}

phase("Test", "Run scoped verification commands");
const testPrompt = promptSlot("test_commands", {
  goal: input,
  surface,
  codex: codexResult,
  required_schema: ".odw/schemas/test-result.schema.json"
}, `
Role:
Scoped test runner.

Input:
{{context}}

Task:
Run the smallest useful verification commands for the changed files and return
structured command results.

Constraints:
- Do not broaden into unrelated full-suite work unless necessary.
- Prefer fast targeted checks.
- Do not edit files.
- Preserve command, status, and output tail for each command.

Output schema:
.odw/schemas/test-result.schema.json

Done criteria:
Every meaningful verification command has status and output tail.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
const tests = await agent(testPrompt, {
  label: "verification commands",
  phase: "Test",
  agentType: "odw-test-runner",
  schema: ".odw/schemas/test-result.schema.json",
  schemaDescription: schemaDescriptions.testResult,
  retry: { maxAttempts: 2 }
});

if (isFailure(tests)) {
  phase("Feedback", "Classify test failure");
  return agent(failureFeedbackPrompt("Test node failure", { goal: input, surface, codex: codexResult, tests }), {
    label: "test failure feedback",
    phase: "Feedback",
    agentType: "odw-failure-analyst",
    schema: ".odw/schemas/error-feedback.schema.json",
    schemaDescription: schemaDescriptions.errorFeedback,
    retry: { maxAttempts: 1 }
  });
}

phase("Verify", "Check the change against the goal");
const verifyPrompt = promptSlot("verify_change", {
  goal: input,
  surface,
  codex: codexResult,
  tests,
  required_schema: ".odw/schemas/verifier.schema.json"
}, `
Role:
Adversarial change verifier.

Input:
{{context}}

Task:
Check whether the change satisfies the goal and whether verification is strong
enough. Reject success claims that are not supported by files, run evidence, or
command output.

Constraints:
- Do not edit files.
- Treat missing tests as needs_more_evidence unless the change is trivial.
- Reject unsupported changed-file or success claims.
- Preserve accepted, rejected, and needs_more_evidence lists.

Output schema:
.odw/schemas/verifier.schema.json

Done criteria:
The final synthesis node can report only evidence-backed facts.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
const verified = await agent(verifyPrompt, {
  label: "change verifier",
  phase: "Verify",
  agentType: "odw-verifier",
  schema: ".odw/schemas/verifier.schema.json",
  schemaDescription: schemaDescriptions.verifier,
  retry: { maxAttempts: 2 }
});

if (isFailure(verified)) {
  phase("Feedback", "Classify verifier failure");
  return agent(failureFeedbackPrompt("Verifier failure", { goal: input, surface, codex: codexResult, tests, verified }), {
    label: "verifier failure feedback",
    phase: "Feedback",
    agentType: "odw-failure-analyst",
    schema: ".odw/schemas/error-feedback.schema.json",
    schemaDescription: schemaDescriptions.errorFeedback,
    retry: { maxAttempts: 1 }
  });
}

phase("Synthesize", "Return the final ship report");
const synthesisPrompt = promptSlot("synthesize_report", {
  goal: input,
  codex: codexResult,
  tests,
  verified,
  required_schema: ".odw/schemas/synthesis.schema.json"
}, `
Role:
Final ship report synthesizer.

Input:
{{context}}

Task:
Return changed files, verification, residual risks, and exact next action.

Constraints:
- Use only verified facts.
- Do not overclaim success when verifier requested more evidence.
- Include command evidence when available.
- Be concise and actionable.

Output schema:
.odw/schemas/synthesis.schema.json

Done criteria:
The caller can tell the user what shipped, how it was verified, and what
remains.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
return agent(synthesisPrompt, {
  label: "ship report",
  phase: "Synthesize",
  agentType: "odw-synthesizer",
  schema: ".odw/schemas/synthesis.schema.json",
  schemaDescription: schemaDescriptions.synthesis,
  retry: { maxAttempts: 2 }
});

function failureFeedbackPrompt(kind, context) {
  return promptSlot("failure_feedback", {
    kind,
    context,
    required_schema: ".odw/schemas/error-feedback.schema.json"
  }, `
Role:
Failure feedback analyst.

Input:
{{context}}

Task:
Classify this workflow failure and decide whether retry is safe.

Constraints:
- Do not edit files.
- Preserve command, output tail, category, retryability, and next action.
- If the failure is a schema mismatch, keep the schema issues and node label.
- If account, auth, or missing model blocks progress, mark it non-retryable.

Output schema:
.odw/schemas/error-feedback.schema.json

Done criteria:
The caller has a clear retry or blocker message.

Failure contract:
If blocked or failed, still return .odw/schemas/error-feedback.schema.json.
`);
}

function isFailure(result) {
  return result?.ok === false
    || result?.status === "failed"
    || result?.status === "stopped"
    || Boolean(result?.error);
}
