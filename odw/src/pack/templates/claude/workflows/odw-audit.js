// Open Dynamic Workflow audit starter.
//
// This script runs directly with `odw exec` and can also be loaded by the
// optional Claude Code compatibility path.

const schemaDescriptions = {
  research: "Final response inventories the audit surface and partitions it into safe parallel review batches.",
  securityFinding: "Final response reports evidence-backed findings, clean files, and uncertain areas for one review batch.",
  verifier: "Final response accepts, rejects, or asks for more evidence on reviewed claims.",
  synthesis: "Final response is the verified audit report for the caller."
};

export const meta = {
  name: "odw-audit",
  description: "Fan-out read-only audit with verifier-gated synthesis.",
  phases: [
    { title: "Discover", detail: "Map target files and review batches" },
    { title: "Review", detail: "Fan out independent read-only reviewers" },
    { title: "Verify", detail: "Reject weak or duplicated claims" },
    { title: "Synthesize", detail: "Return the final verified report" }
  ],
  agents: [
    "odw-researcher",
    "odw-security-reviewer",
    "odw-failure-analyst",
    "odw-verifier",
    "odw-synthesizer"
  ],
  schemas: [
    ".odw/schemas/research.schema.json",
    ".odw/schemas/security-finding.schema.json",
    ".odw/schemas/error-feedback.schema.json",
    ".odw/schemas/verifier.schema.json",
    ".odw/schemas/synthesis.schema.json"
  ],
  promptSlots: [
    "discover_inventory",
    "review_batch",
    "verify_findings",
    "synthesize_report"
  ]
};

const input = args;

phase("Discover", "Map target files and review batches");
const inventoryPrompt = promptSlot("discover_inventory", {
  input,
  required_schema: ".odw/schemas/research.schema.json",
  note: "Partition the requested audit surface into independent review batches."
}, `
Role:
Read-only repository discovery worker.

Input:
{{context}}

Task:
Inventory the target files, modules, routes, configuration, and tests relevant
to the requested audit. Partition the surface into independent review batches
that can safely run in parallel. Prefer small batches with clear ownership
boundaries.

Constraints:
- Do not edit files.
- Inspect only evidence that is relevant to the requested target.
- Use exact file paths and concrete observations.
- Do not invent files, risks, or batches that were not inspected.
- Keep the batch count at or below 16.

Output schema:
.odw/schemas/research.schema.json

Done criteria:
The next Review phase has enough small, independent batches to fan out safely.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
const inventory = await agent(inventoryPrompt, {
  label: "inventory",
  phase: "Discover",
  agentType: "odw-researcher",
  schema: ".odw/schemas/research.schema.json",
  schemaDescription: schemaDescriptions.research,
  retry: { maxAttempts: 2 }
});

const batches = Array.isArray(inventory?.batches) && inventory.batches.length > 0
  ? inventory.batches.slice(0, 16)
  : [{ name: "target", input }];

phase("Review", "Fan out independent read-only reviewers");
const reviews = await parallel(batches.map((batch, index) => () =>
  agent(
    promptSlot("review_batch", {
      batch,
      index,
      required_schema: ".odw/schemas/security-finding.schema.json"
    }, `
Role:
Read-only evidence-backed security reviewer.

Input batch:
{{context}}

Task:
Review this batch for concrete auth, permission, injection, secret handling,
unsafe shell, unsafe deserialization, and dangerous file/network access issues.

Constraints:
- Do not edit files.
- No speculation. Report only evidence-backed findings.
- Cite exact files and lines when possible.
- Do not duplicate another finding under a new name.
- If the batch is clean, explain which files were checked in clean_files.

Output schema:
.odw/schemas/security-finding.schema.json

Done criteria:
All accepted findings include file evidence, severity, claim, evidence, and a
small actionable fix hint, or the batch is marked clean.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`),
    {
      id: `review-${index}`,
      label: `review ${batch.name ?? "batch"}`,
      phase: "Review",
      agentType: "odw-security-reviewer",
      schema: ".odw/schemas/security-finding.schema.json",
      schemaDescription: schemaDescriptions.securityFinding,
      retry: { maxAttempts: 2 }
    }
  )
));

phase("Verify", "Reject weak or duplicated claims");
const verifyPrompt = promptSlot("verify_findings", {
  input,
  reviews,
  required_schema: ".odw/schemas/verifier.schema.json"
}, `
Role:
Adversarial verifier.

Input reviews:
{{context}}

Task:
Reject weak, duplicated, stale, unsupported, or out-of-scope claims. Keep only
claims that have independently checkable file evidence.

Constraints:
- Do not add new findings unless they are required to explain rejection.
- Treat missing evidence as rejection.
- Merge duplicates.
- Preserve enough context for the final report to cite the accepted evidence.

Output schema:
.odw/schemas/verifier.schema.json

Done criteria:
Every accepted item has a reason and evidence pointer. Every rejected item has
a concrete rejection reason.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
const verified = await agent(verifyPrompt, {
  label: "verify findings",
  phase: "Verify",
  agentType: "odw-verifier",
  schema: ".odw/schemas/verifier.schema.json",
  schemaDescription: schemaDescriptions.verifier,
  retry: { maxAttempts: 2 }
});

phase("Synthesize", "Return the final verified report");
const synthesisPrompt = promptSlot("synthesize_report", {
  input,
  verified,
  required_schema: ".odw/schemas/synthesis.schema.json"
}, `
Role:
Final report synthesizer.

Input verified claims:
{{context}}

Task:
Return a concise audit report ordered by severity. Include clean areas, residual
risks, and exact next actions.

Constraints:
- Use only verified claims.
- Do not reintroduce rejected claims.
- Keep findings actionable and tied to concrete evidence.
- Make uncertainty explicit when more evidence is needed.

Output schema:
.odw/schemas/synthesis.schema.json

Done criteria:
The caller can understand what was checked, what matters, and what to do next.

Failure contract:
If blocked or failed, return .odw/schemas/error-feedback.schema.json with
category, retryability, next action, and retry prompt.
`);
return agent(synthesisPrompt, {
  label: "final report",
  phase: "Synthesize",
  agentType: "odw-synthesizer",
  schema: ".odw/schemas/synthesis.schema.json",
  schemaDescription: schemaDescriptions.synthesis,
  retry: { maxAttempts: 2 }
});
