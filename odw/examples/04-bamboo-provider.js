// Example 04 — dispatch a Bamboo domestic-provider node through PandaCode.
//
// Dry run (token-free, no executor needed):
//   odw exec --script examples/04-bamboo-provider.js --backend mock --json
// Real run (needs pandacode + provider API key):
//   odw exec --script examples/04-bamboo-provider.js --backend pandacode --json
//
// ODW only dispatches. PandaCode owns Bamboo provider credentials, model calls,
// logs, and token usage.

export const meta = {
  name: "bamboo-provider",
  description: "Dispatch a Bamboo provider node through PandaCode.",
  phases: [{ title: "Bamboo" }],
};

const provider = args?.provider || "deepseek";

phase("Bamboo");
const direct = await agent(
  "Summarize this repository in one concise paragraph.",
  { runtime: "bamboo", provider, label: "bamboo-direct" }
);

const helper = await pandacode.bamboo(
  "List two follow-up checks a reviewer should run.",
  { provider, label: "bamboo-helper" }
);

return {
  ok: true,
  provider,
  direct,
  helper,
};
