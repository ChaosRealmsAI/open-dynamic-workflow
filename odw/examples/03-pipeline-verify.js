// Example 03 — implement then adversarially verify, as a no-barrier pipeline.
//
//   odw exec --script examples/03-pipeline-verify.js --backend mock --json
//
// `pipeline` streams each item through every stage with NO barrier between
// stages: item B can be implementing while item A is already verifying. Each
// stage callback gets (prevResult, originalItem, index). A `schema` makes a node
// return a validated object you can branch on; returning `{ ok:false }` from the
// workflow makes `odw exec` exit non-zero (a CI gate).

export const meta = {
  name: "pipeline-verify",
  description: "Implement each feature, then verify it, per item, no barrier.",
  phases: [{ title: "Implement" }, { title: "Verify" }],
};

const FEATURES = [
  { id: "debounce", file: "debounce.mjs", prompt: "Create debounce.mjs implementing debounce(fn, ms) plus a small inline test. Write the file." },
  { id: "clamp", file: "clamp.mjs", prompt: "Create clamp.mjs implementing clamp(n, lo, hi) plus a small inline test. Write the file." },
];

const VERDICT = {
  type: "object",
  required: ["passed"],
  properties: {
    passed: { type: "boolean" },
    notes: { type: "string" },
  },
};

const results = await pipeline(
  FEATURES,
  (feature) => {
    phase("Implement");
    // No worktree isolation here: the two features touch different files, and the
    // verify stage must READ the implementation, so it has to persist in the
    // working dir. (See example 02 for isolated parallel worktrees.)
    return agent(feature.prompt, { runtime: "codex", label: `impl:${feature.id}` });
  },
  (_impl, feature) => {
    phase("Verify");
    // A claude node returns the structured verdict — coding agents (codex/bamboo)
    // are unreliable at schema output — reading the file the impl just wrote, and
    // it retries if its first reply misses the VERDICT schema.
    return agent(
      `Review the ${feature.id} implementation just written to ${feature.file} in this directory for correctness, then return a verdict.`,
      { runtime: "claude", schema: VERDICT, label: `verify:${feature.id}`, retry: { maxAttempts: 3 } }
    );
  }
);

// On the `mock` backend the verify node can't produce a real verdict, so
// `allPassed` will be false there — that's expected for a dry run. To turn this
// into a CI gate, `return { ok: allPassed, ... }` so `odw exec` exits non-zero
// when verification fails.
const allPassed = results.every((v) => v && v.passed === true);
return { ok: true, allPassed, verdicts: results };
