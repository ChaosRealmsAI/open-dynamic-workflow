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
  { id: "debounce", prompt: "Implement debounce(fn, ms) in debounce.mjs with a test." },
  { id: "clamp", prompt: "Implement clamp(n, lo, hi) in clamp.mjs with a test." },
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
    return agent(feature.prompt, { runtime: "codex", isolation: "worktree", label: `impl:${feature.id}` });
  },
  (impl, feature) => {
    phase("Verify");
    return agent(
      `Review the implementation of ${feature.id} for correctness:\n${impl?.text ?? impl}`,
      { runtime: "codex", schema: VERDICT, label: `verify:${feature.id}` }
    );
  }
);

// On the `mock` backend the verify node can't produce a real verdict, so
// `allPassed` will be false there — that's expected for a dry run. To turn this
// into a CI gate, `return { ok: allPassed, ... }` so `odw exec` exits non-zero
// when verification fails.
const allPassed = results.every((v) => v && v.passed === true);
return { ok: true, allPassed, verdicts: results };
