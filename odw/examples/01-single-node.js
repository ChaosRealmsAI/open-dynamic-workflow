// Example 01 — a single executor node.
//
// Run a dry version (token-free, no executor needed):
//   odw exec --script examples/01-single-node.js --backend mock --json
// Run for real (needs pandacode on PATH or ODW_PANDACODE_BIN):
//   odw exec --script examples/01-single-node.js --backend pandacode --json
//
// No schema -> the node returns the executor's final text (a string).

export const meta = {
  name: "single-node",
  description: "Dispatch one prompt to a codex node and return its final text.",
  phases: [{ title: "Run" }],
};

phase("Run");
const text = await agent(
  "Summarize what this repository does in two sentences.",
  { runtime: "codex", label: "summarize" }
);

return { ok: true, summary: text };
