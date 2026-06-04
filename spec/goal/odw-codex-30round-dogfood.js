export const meta = {
  name: "odw-codex-30round-dogfood",
  description: "Real ODW -> PandaCode -> Codex stress dogfood with parallel, pipeline, synthesis, and Bamboo trial lanes.",
  phases: [
    { title: "Bamboo Trials" },
    { title: "Codex Entry" },
    { title: "Parallel Recon" },
    { title: "Pipeline Checks" },
    { title: "Synthesis" },
    { title: "Exit Review" },
  ],
};

const codexModel = args?.codexModel || "gpt-5.4-mini";
const codexEffort = args?.codexEffort || "low";
const codexTimeout = Number(args?.codexTimeout || 240);
const codexConcurrency = Number(args?.codexConcurrency || 3);
const enableBamboo = args?.enableBamboo !== false;

const codexOptions = (id, label, extra = {}) => ({
  id,
  label,
  runtime: "codex",
  model: codexModel,
  effort: codexEffort,
  permission: "limited",
  timeout: codexTimeout,
  ...extra,
});

const bambooOptions = (id, label, provider, model, extra = {}) => ({
  id,
  label,
  runtime: "bamboo",
  provider,
  model,
  effort: extra.effort || "high",
  permission: "limited",
  timeout: Number(extra.timeout || 180),
  ...extra,
});

function compact(value, limit = 1200) {
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.length > limit ? `${text.slice(0, limit)}...<truncated>` : text;
}

function projectContext() {
  return `You are running inside an isolated git workspace created only for this ODW dogfood.
Do not edit files unless explicitly asked.
Inspect the local files with read-only commands when needed.
Return concise evidence, not broad advice.
Ignore .odw/ and .pandacode/ unless the prompt explicitly asks about ODW orchestration artifacts.

Project task:
${args?.projectTask || "Evaluate the isolated mini project and its CLI/test/docs quality."}`;
}

const bambooTrials = [];
if (enableBamboo) {
  phase("Bamboo Trials", "Attempt high-quality domestic model entry/exit lanes and a lower-cost execution lane.");
  const trialConfigs = args?.bambooTrials || [
    {
      id: "bamboo-entry-high",
      label: "bamboo-entry-high",
      provider: "qwen",
      model: "qwen3.7-max",
      purpose: "High-quality entry planner",
      prompt:
        "Act as the high-quality domestic-model entry planner. Read package.json and README.md if available. Return a concise plan for auditing this isolated project. Do not edit files.",
    },
    {
      id: "bamboo-exec-low",
      label: "bamboo-exec-low",
      provider: "qwen",
      model: "qwen3.6-flash",
      effort: "low",
      purpose: "Lower-cost execution lane",
      prompt:
        "Act as the lower-cost domestic-model execution lane. Inspect src/ and test/ quickly. Return one concrete low-risk implementation observation. Do not edit files.",
    },
    {
      id: "bamboo-exit-high",
      label: "bamboo-exit-high",
      provider: "kimi",
      model: "kimi-k2.6",
      purpose: "High-quality exit reviewer",
      prompt:
        "Act as the high-quality domestic-model exit reviewer. Inspect the repository summary and identify one risk in using cheap execution agents. Do not edit files.",
    },
  ];
  for (const trial of trialConfigs) {
    const result = await agent(
      `${projectContext()}

Domestic model trial:
- purpose: ${trial.purpose}
- provider: ${trial.provider}
- model: ${trial.model}

Task:
${trial.prompt}

Final response: start with BAMBOO_TRIAL ${trial.id}: and summarize whether you could run, what you inspected, and any blocker.`,
      bambooOptions(trial.id, trial.label, trial.provider, trial.model, {
        effort: trial.effort || "high",
        timeout: trial.timeout || 180,
      })
    );
    bambooTrials.push({ trial, result });
  }
}

phase("Codex Entry", "Use Codex as the reliable entry planner for the isolated task.");
const entry = await agent(
  `${projectContext()}

Entry task:
Inspect package.json, README.md, src/, and test/ at a high level.
Return:
1. the project shape;
2. three audit dimensions worth parallelizing;
3. one validator command to use later.

Final response: start with CODEX_ENTRY and stay under 160 words.`,
  codexOptions("codex-entry", "codex-entry")
);

const reconDimensions = [
  ["package-surface", "Inspect package.json scripts and dependency surface."],
  ["cli-contract", "Inspect the CLI command behavior described by README and src/cli.js."],
  ["parser", "Inspect parsing logic and edge cases."],
  ["formatter", "Inspect formatter/output behavior."],
  ["storage", "Inspect JSON storage/read-write assumptions."],
  ["errors", "Inspect error handling and user-facing messages."],
  ["tests-unit", "Inspect unit test coverage and assertions."],
  ["tests-integration", "Inspect integration-style gaps."],
  ["docs-readme", "Inspect README accuracy and missing workflow details."],
  ["docs-examples", "Inspect examples and command snippets."],
  ["security-paths", "Inspect path handling and workspace escape risks."],
  ["data-model", "Inspect task object fields and schema assumptions."],
  ["performance", "Inspect likely performance hot spots for many tasks."],
  ["concurrency", "Inspect behavior if commands run concurrently."],
  ["observability", "Inspect logs/output useful for debugging."],
  ["migration", "Inspect versioning or migration gaps."],
  ["ux-new-user", "Inspect first-run and onboarding clarity."],
  ["ux-failure", "Inspect failure recovery and next-step guidance."],
  ["maintainability", "Inspect module boundaries and naming."],
  ["release", "Inspect packaging/release readiness."],
];

phase("Parallel Recon", "Run twenty independent Codex inspection lanes.");
const recon = await parallel(
  reconDimensions.map(([id, instruction], index) => () =>
    agent(
      `${projectContext()}

Parallel recon lane ${index + 1}/20: ${id}
${instruction}

Constraints:
- Read files as needed.
- Do not edit files.
- Return exactly one concise paragraph.
- Start the final response with RECON ${id}:`,
      codexOptions(`recon-${id}`, `recon-${id}`)
    )
  ),
  { label: "parallel-recon", max: codexConcurrency }
);

const pipelineItems = [
  { id: "cli-flow", files: "src/cli.js test/cli.test.js", focus: "CLI user flow and tests" },
  { id: "task-store", files: "src/store.js test/store.test.js", focus: "JSON task persistence" },
  { id: "reporting", files: "src/report.js README.md", focus: "Output/report ergonomics" },
  { id: "docs-contract", files: "README.md package.json", focus: "Documented command contract" },
  { id: "quality-gates", files: "package.json test/", focus: "Validation commands and quality gates" },
];

phase("Pipeline Checks", "Run five two-stage Codex pipelines: inspect then verify.");
const pipelineResults = await pipeline(
  pipelineItems,
  async (item) =>
    agent(
      `${projectContext()}

Pipeline inspect stage for ${item.id}.
Focus: ${item.focus}
Files: ${item.files}

Read the files and return the strongest evidence-backed observation.
Do not edit files.
Final response must start with PIPE_INSPECT ${item.id}:`,
      codexOptions(`pipe-${item.id}-inspect`, `pipe-${item.id}-inspect`)
    ),
  async (inspection, item) =>
    agent(
      `${projectContext()}

Pipeline verify stage for ${item.id}.
Prior inspection:
${compact(inspection)}

Task:
Challenge the prior inspection. Say whether it is actionable, overstated, or missing evidence.
Do not edit files.
Final response must start with PIPE_VERIFY ${item.id}:`,
      codexOptions(`pipe-${item.id}-verify`, `pipe-${item.id}-verify`)
    )
);

phase("Synthesis", "Use Codex to synthesize the parallel and pipeline evidence.");
const synthesis = await agent(
  `${projectContext()}

Entry:
${compact(entry)}

Bamboo trials:
${compact(bambooTrials, 1800)}

Parallel recon results:
${compact(recon, 5000)}

Pipeline results:
${compact(pipelineResults, 4000)}

Task:
Synthesize the evidence into:
1. what ODW made easier than direct codexctl;
2. what ODW made harder;
3. one concrete ODW product improvement worth implementing;
4. whether domestic high/low model lanes executed or were blocked.

Final response must start with SYNTHESIS:`,
  codexOptions("codex-synthesis", "codex-synthesis")
);

phase("Exit Review", "Use Codex as final exit reviewer over the run evidence.");
const exitReview = await agent(
  `${projectContext()}

Synthesis:
${compact(synthesis, 3000)}

Task:
Act as an exit reviewer. Identify the top ODW improvement candidate and the key evidence needed before committing it.
Also compare ODW orchestration with direct codexctl for this 30+ node workload.
Do not edit files.
Final response must start with EXIT_REVIEW:`,
  codexOptions("codex-exit-review", "codex-exit-review", {
    model: args?.exitCodexModel || codexModel,
    effort: args?.exitCodexEffort || "medium",
  })
);

return {
  ok: true,
  isolatedWorkspace: globalThis.cwd,
  requestedCodexRounds: 33,
  countedCodexNodes: {
    entry: 1,
    parallelRecon: reconDimensions.length,
    pipeline: pipelineItems.length * 2,
    synthesis: 1,
    exitReview: 1,
  },
  bambooTrials,
  entry,
  recon,
  pipelineResults,
  synthesis,
  exitReview,
};
