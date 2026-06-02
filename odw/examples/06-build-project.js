// Example 06 — build a real project end-to-end (design → implement → review → verify).
//
// A reusable template: codex implements in the working dir, a reviewer (claude)
// reads the code, codex applies fixes and runs the test command until green.
// This is the shape used to build the dogfooded KV-store (Python) and roman
// numeral (Rust) projects.
//
//   Dry run (token-free graph preview):
//     odw exec --script examples/06-build-project.js --backend mock --json
//
//   Real run — point --path at a fresh (ideally empty) project dir so codex
//   writes there, and pass the spec + test command via --input:
//     odw exec --path ./my-project --backend pandacode \
//       --script examples/06-build-project.js \
//       --input '{"spec":"a CLI that reverses lines of stdin","test":"python3 -m pytest -q"}'
//
// codex nodes default to least-privilege (sandboxed to --path); a real coding
// build legitimately runs for minutes, so codex's per-node timeout floors at 10m.
export const meta = {
  name: "build-project",
  description: "Design, implement, review, and verify a small project in the working directory",
  phases: [{ title: "Implement" }, { title: "Review" }, { title: "Verify" }],
};

const SPEC = (args && args.spec) || "a small command-line tool, your choice, with unit tests";
const TEST = (args && args.test) || "echo 'set args.test to your test command'";

// A no-schema node returns the final text on a real run, but a token-free mock
// dry-run returns a metadata object — coerce both to a readable string so dry-run
// output never prints "[object Object]".
const asText = (r) => (typeof r === "string" ? r : (r && r.text) || JSON.stringify(r));

phase("Implement");
const impl = asText(await agent(
  `In the CURRENT directory, implement: ${SPEC}.
Write real, runnable code plus unit tests. Keep dependencies minimal. Make the tests pass with: ${TEST}`,
  { label: "implement (codex)", runtime: "codex" }
));

phase("Review");
const review = asText(await agent(
  `Review the code just written in this directory for correctness bugs and missing edge cases. List concrete issues with file:line. Do NOT edit files.`,
  { label: "review (claude)", runtime: "claude" }
));

phase("Verify");
const verify = asText(await agent(
  `Apply the fixes from this review, then run \`${TEST}\` in this directory and report exact pass/fail counts. Iterate until green or blocked.\n\nReview:\n${review.slice(0, 1500)}`,
  { label: "fix+verify (codex)", runtime: "codex" }
));

return { implemented: impl.length > 0, reviewLen: review.length, verify: verify.slice(0, 240) };
