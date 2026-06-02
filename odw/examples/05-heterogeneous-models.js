// Example 05 — fan one task across HETEROGENEOUS models, then synthesize.
//
// This is ODW's headline difference from the built-in Workflow tool: every node
// can target its own runtime/provider/model, so a single parallel() compares
// answers from several domestic models at once and a claude node reconciles them.
//
//   Dry run (token-free, no executor needed):
//     odw exec --script examples/05-heterogeneous-models.js --backend mock --json
//
//   Real run (needs pandacode + provider keys):
//     odw exec --script examples/05-heterogeneous-models.js --backend pandacode \
//       --input '{"question":"...","models":{"deepseek":"deepseek-chat","qwen":"qwen-max","kimi":"kimi-k2"}}'
//
// The execution-graph report (printed at the end of a real run) shows each
// node's resolved model + token count side by side.
export const meta = {
  name: "heterogeneous-models",
  description: "Answer one question with several different models in parallel, then reconcile with claude",
  phases: [{ title: "Survey" }, { title: "Synthesize" }],
};

const M = (args && args.models) || {};
const QUESTION =
  (args && args.question) ||
  "What is the single biggest risk in a distributed rate limiter, and the simplest mitigation?";

const PANEL = [
  { provider: "deepseek", model: M.deepseek },
  { provider: "qwen", model: M.qwen },
  { provider: "kimi", model: M.kimi },
];

phase("Survey");
const answers = await parallel(
  PANEL.map((p) => () =>
    agent(`${QUESTION}\nAnswer in 2 sentences.`, {
      label: `ask:${p.provider}`,
      phase: "Survey",
      runtime: "bamboo",
      provider: p.provider,
      model: p.model,
    }).then((text) => ({ provider: p.provider, text: String(text) }))
  )
);

phase("Synthesize");
const brief = answers
  .filter((a) => a && a.text)
  .map((a) => `[${a.provider}] ${a.text.slice(0, 400)}`)
  .join("\n\n");
const consensus = String(
  await agent(
    `Three models answered the same question. Reconcile them into one best answer and note any disagreement.\n\n${brief}`,
    { label: "synthesize", phase: "Synthesize", runtime: "claude" }
  )
);

return { models: PANEL.map((p) => p.provider), consensus: consensus.slice(0, 400) };
