// Example 02 — fan out independent edits in isolated worktrees, collect results.
//
//   odw exec --script examples/02-parallel-fanout.js --backend mock --json
//
// `parallel` is a barrier: it waits for all thunks. A thunk that throws becomes
// `null` (the batch never rejects), so filter before use. Each node runs in its
// own throwaway git worktree so concurrent file edits never collide; the captured
// diff comes back on `result.worktree` (requires cwd to be a git repo for real runs).

export const meta = {
  name: "parallel-fanout",
  description: "Implement three independent modules concurrently, each isolated.",
  phases: [{ title: "Implement" }],
};

const TASKS = [
  { id: "title", prompt: "Implement titleCase(str) in titleCase.mjs and test it." },
  { id: "range", prompt: "Implement range(start,end) in range.mjs and test it." },
  { id: "slug", prompt: "Implement slugify(str) in slug.mjs and test it." },
];

phase("Implement");
const results = await parallel(
  TASKS.map((t) => () =>
    agent(t.prompt, { runtime: "codex", isolation: "worktree", label: `impl:${t.id}` })),
  { label: "fanout" }
);

const done = results
  .map((r, i) => ({ task: TASKS[i].id, result: r }))
  .filter((x) => x.result !== null);

return { ok: done.length === TASKS.length, done };
