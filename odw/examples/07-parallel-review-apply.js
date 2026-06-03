// Example 07 — parallel Codex worktrees, structured review gate, atomic landing.
//
// This is the reusable large-project shape:
//
//   optional high-level request planner
//     -> parallel implementation worktrees
//     -> reviewWorktreeDiffs(...) in a temporary candidate worktree
//     -> applyWorktreeDiffs(...) atomically only after approve
//     -> final verification in the main working directory
//
// It intentionally lands changes into the cwd when the review gate approves.
// Run it from a disposable git repo or the project you actually want to change:
//
//   mkdir /tmp/odw-example && cd /tmp/odw-example && git init && git commit --allow-empty -m init
//   odw exec --script /path/to/odw/examples/07-parallel-review-apply.js --backend mock --json
//
// Real run:
//
//   odw exec --script /path/to/odw/examples/07-parallel-review-apply.js \
//     --backend pandacode \
//     --input '{"test":"npm test","tasks":[{"id":"docs","file":"docs/agent-loop.md","prompt":"Create docs/agent-loop.md explaining the agent loop."}]}'
//
// Lower decision-cost run: pass `request` or `spec` instead of `tasks`; the
// starter plans owned task files first, then reuses the same review/apply gate.

export const meta = {
  name: "parallel-review-apply",
  description: "Implement independent tasks in worktrees, review the combined candidate, then land atomically.",
  phases: [
    { title: "Plan" },
    { title: "Implement" },
    { title: "Review Gate" },
    { title: "Repair" },
    { title: "Land" },
    { title: "Verify" },
  ],
};

export default async function workflow() {
  const DEFAULT_TASKS = [
    {
      id: "owner-loop",
      file: "docs/owner-loop.md",
      prompt: "Create docs/owner-loop.md explaining how owner comments become AI implementation tasks.",
    },
    {
      id: "review-policy",
      file: "docs/review-policy.md",
      prompt: "Create docs/review-policy.md explaining approve/reject/needs_owner review outcomes.",
    },
  ];

  const TEST = args?.test || "echo 'no test command configured'";
  const REQUEST = String(args?.request || args?.spec || args?.goal || "").trim();
  const TASK_PLAN_SCHEMA = {
    title: "task-plan.schema.json",
    type: "object",
    required: ["status", "summary", "tasks"],
    properties: {
      status: { enum: ["planned"] },
      summary: { type: "string" },
      tasks: {
        type: "array",
        items: {
          type: "object",
          required: ["id", "prompt"],
          properties: {
            id: { type: "string" },
            title: { type: "string" },
            file: { type: "string" },
            files: { type: "array", items: { type: "string" } },
            prompt: { type: "string" },
            verify: { type: "string" },
            runtime: { type: "string" },
            permission: { type: "string" },
          },
        },
      },
      risks: { type: "array", items: { type: "string" } },
    },
  };
  const normalizePlannedTask = (task, index) => {
    const normalized = {
      ...task,
      id: String(task?.id || `task-${index + 1}`).trim(),
      prompt: String(task?.prompt || "").trim(),
    };
    if (Array.isArray(task?.files)) {
      normalized.files = task.files;
    }
    if (Object.prototype.hasOwnProperty.call(task || {}, "file")) {
      normalized.file = task.file;
    }
    return normalized;
  };
  let planned = null;
  let TASKS = Array.isArray(args?.tasks) && args.tasks.length ? args.tasks : null;
  if (!TASKS && REQUEST) {
    phase("Plan", "Decompose the high-level request into owned parallel tasks.");
    planned = await agent(
      `Decompose this owner request into independently owned implementation tasks for the ODW parallel-review-apply starter.

Owner request:
${REQUEST}

Run context:
${args?.context || REQUEST}

Verification command:
${TEST}

Return 2-6 tasks when practical. Each task must:
- have a stable short kebab-case id;
- declare repo-relative owned files with file or files;
- avoid duplicate file ownership across parallel tasks;
- avoid .git, .odw, .pandacode, node_modules, absolute paths, and .. paths;
- include a concrete prompt that states public API/data contracts when relevant;
- include tests/docs as owned files when the request needs them;
- keep dependent public entrypoints, tests, and docs explicit rather than implied.

If the request is broad, choose a small coherent first slice that can be reviewed and verified safely.`,
      {
        id: "plan-tasks",
        label: "plan-tasks",
        runtime: args?.plannerRuntime || "codex",
        permission: args?.plannerPermission || "limited",
        schema: TASK_PLAN_SCHEMA,
        schemaDescription: "Final response is a structured implementation task plan for the parallel-review-apply starter.",
        retry: { maxAttempts: Math.max(1, Math.min(4, Number(args?.plannerMaxAttempts || 3))) },
      }
    );
    if (!planned || planned?.ok === false || !Array.isArray(planned.tasks) || planned.tasks.length === 0) {
      return {
        ok: false,
        error: {
          category: "planning_failed",
          message: "The high-level request planner did not return a usable task list.",
        },
        planned,
        hint:
          "Pass explicit args.tasks, provide a narrower args.request/spec, or retry with a plannerRuntime that handles structured JSON reliably.",
      };
    }
    TASKS = planned.tasks.map(normalizePlannedTask);
  }
  TASKS = TASKS || DEFAULT_TASKS;
  const defaultReviewRounds = TASKS.length >= 3 ? 3 : 2;
  const maxReviewRounds = Math.max(1, Math.min(4, Number(args?.maxReviewRounds || defaultReviewRounds)));
  const strictTaskFileBoundaries = args?.strictTaskFileBoundaries !== false;
  const allowDirtyTaskFiles = args?.allowDirtyTaskFiles === true;
  const allowDuplicateTaskFiles = args?.allowDuplicateTaskFiles === true;
  const allowUndeclaredTaskFiles = args?.allowUndeclaredTaskFiles === true;

  const taskIdEntries = TASKS.map((task, index) => ({
    index,
    id: String(task?.id ?? "").trim(),
    file: task?.file || null,
  }));
  const missingTaskIds = taskIdEntries
    .filter((entry) => !entry.id)
    .map((entry) => ({
      index: entry.index,
      file: entry.file,
    }));
  const taskIdOwners = new Map();
  for (const entry of taskIdEntries) {
    if (!entry.id) {
      continue;
    }
    const owners = taskIdOwners.get(entry.id) || [];
    owners.push({ index: entry.index, file: entry.file });
    taskIdOwners.set(entry.id, owners);
  }
  const duplicateTaskIds = [...taskIdOwners.entries()]
    .filter(([, owners]) => owners.length > 1)
    .map(([id, owners]) => ({ id, owners }));
  if (missingTaskIds.length > 0 || duplicateTaskIds.length > 0) {
    return {
      ok: false,
      error: {
        category: "invalid_task_ids",
        message:
          "Every parallel task must declare a stable unique id before worktrees can be created.",
      },
      missingTaskIds,
      duplicateTaskIds,
      hint:
        "Assign each task a short unique id. ODW uses task ids for node keys, sessions, repair history, and reports.",
    };
  }

  const declaredTaskFileValues = (task) => {
    const values = [];
    if (Object.prototype.hasOwnProperty.call(task || {}, "file")) {
      values.push(task.file);
    }
    if (Array.isArray(task?.files)) {
      values.push(...task.files);
    }
    return values;
  };

  const normalizeTaskFile = (value) => {
    const raw = String(value ?? "").trim();
    if (!raw) {
      return { raw, path: null, error: "empty_path" };
    }
    if (raw.includes("\0")) {
      return { raw, path: null, error: "nul_byte" };
    }
    if (raw.startsWith("/") || raw.startsWith("\\") || /^[A-Za-z]:[\\/]/.test(raw)) {
      return { raw, path: null, error: "absolute_path" };
    }
    if (raw.includes("\\")) {
      return { raw, path: null, error: "backslash_path" };
    }
    const parts = raw.split("/").filter((part) => part && part !== ".");
    if (parts.length === 0) {
      return { raw, path: null, error: "empty_path" };
    }
    if (parts.some((part) => part === "..")) {
      return { raw, path: null, error: "path_escape" };
    }
    const blocked = parts.find((part) => [".git", ".odw", ".pandacode", "node_modules"].includes(part));
    if (blocked) {
      return { raw, path: null, error: "reserved_path", segment: blocked };
    }
    return { raw, path: parts.join("/"), error: null };
  };

  const declaredTaskFileEntries = (task, index) =>
    declaredTaskFileValues(task).map((value) => ({
      ...normalizeTaskFile(value),
      task: task.id,
      index,
    }));

  const declaredFilesByTask = new Map(
    TASKS.map((task, index) => [task.id, declaredTaskFileEntries(task, index)])
  );

  const taskFiles = (task) =>
    [...new Set((declaredFilesByTask.get(task.id) || [])
      .filter((entry) => !entry.error && entry.path)
      .map((entry) => entry.path))];

  const invalidTaskFiles = [...declaredFilesByTask.values()]
    .flat()
    .filter((entry) => entry.error)
    .map((entry) => ({
      task: entry.task,
      index: entry.index,
      file: entry.raw,
      error: entry.error,
      segment: entry.segment || null,
    }));
  if (invalidTaskFiles.length > 0) {
    return {
      ok: false,
      error: {
        category: "invalid_task_files",
        message:
          "Declared task files must be normalized repo-relative paths outside ODW/PandaCode/internal generated directories.",
      },
      invalidTaskFiles,
      hint:
        "Use POSIX-style repo-relative paths like src/api.ts. Do not use absolute paths, '..', backslashes, .git, .odw, .pandacode, or node_modules.",
    };
  }

  const invalidTaskPrompts = TASKS
    .map((task, index) => ({
      index,
      id: task.id,
      files: taskFiles(task),
      type: typeof task?.prompt,
      prompt: task?.prompt,
    }))
    .filter((entry) => typeof entry.prompt !== "string" || entry.prompt.trim().length === 0)
    .map((entry) => ({
      index: entry.index,
      id: entry.id,
      files: entry.files,
      type: entry.type,
    }));
  if (invalidTaskPrompts.length > 0) {
    return {
      ok: false,
      error: {
        category: "invalid_task_prompts",
        message:
          "Every parallel task must declare a non-empty prompt before worktrees can be created.",
      },
      invalidTaskPrompts,
      hint:
        "Write a concrete task prompt for each task. Empty or non-string prompts make implementation nodes ambiguous and unsafe.",
    };
  }

  const taskBrief = TASKS.map(
    (task) => `- ${task.id}: ${taskFiles(task).join(", ") || "(files from prompt)"} — ${task.prompt}`
  ).join("\n");
  const runContext =
    args?.context ||
    "Large-project default: land low-risk, internally consistent changes with verification evidence.";
  const reviewContext = `Caller-provided context and task prompts are the owner-provided product intent for this run.

Run context:
${runContext}

Planned tasks:
${taskBrief}`;
  const reviewCriteria = args?.criteria || [
    "Treat the run context and task prompts as the acceptance intent for this batch.",
    "Approve when the candidate satisfies that stated intent, applies cleanly, and has adequate verification evidence.",
    "Use needs_owner only when the candidate makes a consequential product choice not present in the run context or task prompts, or when the stated intent conflicts with repository evidence.",
    "Reject when there are blockers, failed verification, semantic conflicts, or unsafe/unrelated edits.",
  ];

  const implementationPrompt = (task, repairFeedback) => `Batch context:
${runContext}

Planned task contracts:
${taskBrief}

Current task (${task.id}):
${task.prompt}

${repairFeedback ? `Review feedback to address before returning:\n${repairFeedback}

When feedback references files owned by other tasks, treat those references as evidence only. Repair only this task's declared file list and preserve the original task intent.
` : ""}
Constraints:
- Only edit the files needed for this task${taskFiles(task).length ? `: ${taskFiles(task).join(", ")}` : ""}.
- Keep the change independently reviewable.
- Align this task with the run context and sibling task contracts above; do not invent a different public API, data shape, file name, or acceptance contract.
- Tests and docs must target the declared task files and exports from the planned tasks. Do not invent package entrypoints, public modules, or skip paths unless that file is declared in a task's ownership list.
- If verification cannot pass in this isolated worktree because dependent task files are absent, still write the real intended tests/docs and report that dependency honestly; do not skip tests to make verification pass.
- Do not claim defaults or generated files that are not directly true from the task context or project evidence.
- Run this verification if relevant: ${task.verify || TEST}
- Final response: one concise sentence with changed files and verification result.`;

  const undeclaredTaskFiles = TASKS
    .map((task, index) => ({
      index,
      id: task.id,
      files: taskFiles(task),
    }))
    .filter((entry) => entry.files.length === 0)
    .map((entry) => ({
      index: entry.index,
      id: entry.id,
    }));
  if (!allowUndeclaredTaskFiles && undeclaredTaskFiles.length > 0) {
    return {
      ok: false,
      error: {
        category: "undeclared_task_files",
        message:
          "Every parallel task must declare task.file or task.files so ODW can enforce ownership and target repairs.",
      },
      undeclaredTaskFiles,
      hint:
        "Declare each task's owned files, split exploratory work into a planning step, or pass allowUndeclaredTaskFiles:true only with explicit owner intent.",
    };
  }

  const fileOwner = new Map();
  const fileOwners = new Map();
  for (const task of TASKS) {
    for (const file of taskFiles(task)) {
      if (!fileOwner.has(file)) {
        fileOwner.set(file, task);
      }
      const owners = fileOwners.get(file) || [];
      owners.push(task);
      fileOwners.set(file, owners);
    }
  }

  const duplicateTaskFiles = [...fileOwners.entries()]
    .filter(([, owners]) => owners.length > 1)
    .map(([file, owners]) => ({
      file,
      tasks: owners.map((task) => task.id),
    }));
  if (!allowDuplicateTaskFiles && duplicateTaskFiles.length > 0) {
    return {
      ok: false,
      error: {
        category: "duplicate_task_files",
        message:
          "Multiple parallel tasks declare the same file. This starter expects independently owned task files.",
      },
      duplicateTaskFiles,
      hint:
        "Merge those tasks, run them serially, or pass allowDuplicateTaskFiles:true only when overlapping patches are intentional and reviewable.",
    };
  }

  const startSnapshot = captureMainWorktreeSnapshot({ label: "starter-preflight" });
  const dirtyTaskFiles = allowDirtyTaskFiles
    ? []
    : startSnapshot.files.filter((file) => fileOwner.has(file));
  if (dirtyTaskFiles.length > 0) {
    return {
      ok: false,
      error: {
        category: "dirty_task_files",
        message:
          "Task files already have uncommitted changes. Isolated worktrees branch from HEAD and would not see those changes.",
      },
      dirtyTaskFiles,
      hint:
        "Commit or stash the listed task files before running this starter again, or pass allowDirtyTaskFiles:true with explicit owner intent.",
    };
  }

  const runImplementationRound = async (round, repairFeedback = "", roundTasks = TASKS) => {
    const isRepair = round > 1;
    const activeTasks = Array.isArray(roundTasks) && roundTasks.length ? roundTasks : TASKS;
    phase(
      isRepair ? "Repair" : "Implement",
      isRepair
        ? `Redo ${activeTasks.length} rejected task(s) from clean worktrees using review feedback (round ${round}/${maxReviewRounds}).`
        : "Fan out independent Codex tasks into isolated worktrees."
    );
    const results = await parallel(
      activeTasks.map((task) => () =>
        agent(implementationPrompt(task, repairFeedback), {
          id: isRepair ? `${task.id}-repair-${round - 1}` : task.id,
          label: isRepair ? `repair:${task.id}` : `impl:${task.id}`,
          runtime: task.runtime || "codex",
          isolation: "worktree",
          permission: task.permission || "max",
          // Mock backend only: makes the dry run produce a real captured diff.
          mockWriteFile: task.mockFile || task.file || task.files?.[0],
          mockFail: Boolean(task.mockFail),
        })
      ),
      { label: isRepair ? `repair-${round - 1}` : "implement" }
    );
    const annotated = activeTasks.map((task, index) => ({ task, result: results[index] }));
    const candidates = annotated
      .filter(({ result }) => result?.worktree?.changed)
      .map(({ task, result }) => ({
        ...result,
        taskId: task.id,
        taskFile: task.file || task.files?.[0] || null,
        taskFiles: taskFiles(task),
      }));
    const failedTasks = annotated
      .filter(({ result }) => !result || result?.ok === false)
      .map(({ task, result }) => ({
        task,
        message:
          result?.error?.message ||
          result?.feedback?.user_message ||
          result?.text ||
          "implementation node failed or returned no result",
      }));
    const scopeIssues = strictTaskFileBoundaries
      ? candidates.flatMap((candidate) => {
          const task = activeTasks.find((item) => item.id === candidate.taskId);
          const allowed = new Set(taskFiles(task));
          if (allowed.size === 0) {
            return [];
          }
          return (candidate.worktree?.files || [])
            .filter((file) => !allowed.has(file))
            .map((file) => ({
              task,
              file,
              ownerTask: fileOwner.get(file) || null,
            }));
        })
      : [];
    log(
      (isRepair ? "repair " : "") +
        "candidate files=" +
        candidates.flatMap((result) => result.worktree.files).join("|")
    );
    return { activeTasks, annotated, candidates, failedTasks, scopeIssues, results };
  };

  const implementationFeedback = (issues) => {
    const failed = (issues?.failedTasks || [])
      .map((item) => `failed_task: ${item.task.id} (${item.task.file || "no file"}) — ${item.message}`)
      .join("\n");
    const scope = (issues?.scopeIssues || [])
      .map((item) => {
        const owner = item.ownerTask ? `; owned_by=${item.ownerTask.id}` : "";
        return `scope_violation: task ${item.task.id} touched ${item.file}${owner}`;
      })
      .join("\n");
    return `Pre-review implementation gate blocked this batch before reviewer agents ran.
${failed}
${scope}

Repair only the listed task files. If multiple tasks need coordinated changes, keep each task inside its declared file list or set args.strictTaskFileBoundaries=false with explicit owner intent.`.slice(0, args?.maxRepairFeedbackChars || 12000);
  };

  const implementationIssues = (implementation) => {
    const tasks = new Map();
    for (const item of implementation?.failedTasks || []) {
      tasks.set(item.task.id, item.task);
    }
    for (const issue of implementation?.scopeIssues || []) {
      tasks.set(issue.task.id, issue.task);
      if (issue.ownerTask) {
        tasks.set(issue.ownerTask.id, issue.ownerTask);
      }
    }
    return {
      failedTasks: implementation?.failedTasks || [],
      scopeIssues: implementation?.scopeIssues || [],
      tasks: [...tasks.values()],
    };
  };

  const reviewFeedback = (gate) => {
    const reviewLines = (gate?.reviews || [])
      .map((review) => {
        const parts = [
          `Reviewer ${review.reviewer || "review"} decision=${review.decision || "unknown"}`,
          review.summary ? `summary: ${review.summary}` : "",
          ...(review.blockers || []).map((item) => `blocker: ${item}`),
          ...(review.risks || []).map((item) => `risk: ${item}`),
          ...(review.verification || []).map((item) => `verification: ${item}`),
        ].filter(Boolean);
        return parts.join("\n");
      })
      .join("\n\n");
    const ownerQuestions = (gate?.owner_questions || []).map((item) => `owner_question: ${item}`).join("\n");
    return `Previous review decision: ${gate?.decision || "unknown"}
${(gate?.blockers || []).map((item) => `Blocking issue: ${item}`).join("\n")}
${ownerQuestions}

Reviewer evidence:
${reviewLines}`.slice(0, args?.maxRepairFeedbackChars || 12000);
  };

  const reviewBlockers = (gate) => [
    ...(gate?.blockers || []),
    ...(gate?.reviews || []).flatMap((review) => review.blockers || []),
  ].filter(Boolean);

  const uniqueTasks = (items) => {
    const seen = new Map();
    for (const task of items || []) {
      if (task?.id && !seen.has(task.id)) {
        seen.set(task.id, task);
      }
    }
    return [...seen.values()];
  };

  const primaryTasksForBlocker = (blocker, tasksWithFiles) => {
    const text = String(blocker || "");
    let bestIndex = Infinity;
    const matched = [];
    for (const task of tasksWithFiles) {
      for (const file of taskFiles(task)) {
        const index = text.indexOf(file);
        if (index < 0) {
          continue;
        }
        if (index < bestIndex) {
          bestIndex = index;
          matched.length = 0;
        }
        if (index === bestIndex) {
          matched.push(task);
        }
      }
    }
    return uniqueTasks(matched);
  };

  const tasksForReviewRepair = (gate) => {
    const blockers = reviewBlockers(gate);
    const tasksWithFiles = TASKS.filter((task) => taskFiles(task).length > 0);
    if (blockers.length === 0 || tasksWithFiles.length === 0) {
      return TASKS;
    }
    const primaryMatched = uniqueTasks(
      blockers.flatMap((blocker) => primaryTasksForBlocker(blocker, tasksWithFiles))
    );
    if (primaryMatched.length > 0) {
      return primaryMatched;
    }
    const fallbackMatched = uniqueTasks(
      tasksWithFiles.filter((task) =>
        blockers.some((blocker) => taskFiles(task).some((file) => String(blocker).includes(file)))
      )
    );
    return fallbackMatched.length > 0 ? fallbackMatched : TASKS;
  };

  const candidateTouchesTask = (candidate, task) =>
    taskFiles(task).some((file) => candidate?.worktree?.files?.includes(file));

  const candidateFiles = (items) =>
    [...new Set((items || []).flatMap((candidate) => candidate?.worktree?.files || []))];
  const summarizeGate = (round, label, value) => ({
    round,
    label,
    decision: value?.decision,
    applyReady: value?.applyReady === true,
    files: value?.files || [],
    reviewers: (value?.reviews || []).map((review) => ({
      reviewer: review.reviewer,
      decision: review.decision,
      summary: review.summary,
      blockers: review.blockers || [],
      risks: review.risks || [],
      owner_questions: review.owner_questions || [],
      verification: review.verification || [],
    })),
    blockers: value?.blockers || [],
    risks: value?.risks || [],
    owner_questions: value?.owner_questions || [],
    verification: value?.verification || [],
  });

  const history = [];
  if (planned) {
    history.push({
      step: "plan",
      status: planned.status,
      summary: planned.summary,
      tasks: TASKS.map((task) => ({
        id: task.id,
        files: taskFiles(task),
      })),
      risks: planned.risks || [],
    });
  }
  let implementation = await runImplementationRound(1);
  let candidates = implementation.candidates;
  history.push({
    step: "implement",
    round: 1,
    tasks: TASKS.map((task) => task.id),
    files: candidateFiles(candidates),
  });

  if (candidates.length === 0) {
    return { ok: false, error: "no captured worktree changes", history, results: implementation.results };
  }

  let gate = null;
  for (let round = 1; round <= maxReviewRounds; round += 1) {
    const preReviewIssues = implementationIssues(implementation);
    if (preReviewIssues.tasks.length > 0) {
      history.push({
        step: "pre_review_block",
        round,
        failed_tasks: preReviewIssues.failedTasks.map((item) => ({
          id: item.task.id,
          file: item.task.file || null,
          files: taskFiles(item.task),
          message: item.message,
        })),
        scope_issues: preReviewIssues.scopeIssues.map((item) => ({
          task: item.task.id,
          file: item.file,
          owner_task: item.ownerTask?.id || null,
        })),
      });
      if (round >= maxReviewRounds) {
        return {
          ok: false,
          error: {
            category: "implementation_pre_review_blocked",
            message: "Implementation tasks failed or crossed task file boundaries before review.",
          },
          history,
          results: implementation.results,
        };
      }
      const repairTasks = preReviewIssues.tasks;
      log("pre-review repairing tasks=" + repairTasks.map((task) => task.id).join("|"));
      const retainedCandidates =
        repairTasks.length === TASKS.length
          ? []
          : candidates.filter((candidate) => !repairTasks.some((task) => candidateTouchesTask(candidate, task)));
      history.push({
        step: "repair_plan",
        reason: "pre_review_block",
        round: round + 1,
        tasks: repairTasks.map((task) => task.id),
        retained_files: candidateFiles(retainedCandidates),
      });
      implementation = await runImplementationRound(round + 1, implementationFeedback(preReviewIssues), repairTasks);
      candidates = [...retainedCandidates, ...implementation.candidates];
      history.push({
        step: "repair",
        reason: "pre_review_block",
        round: round + 1,
        tasks: repairTasks.map((task) => task.id),
        files: candidateFiles(implementation.candidates),
        candidate_files: candidateFiles(candidates),
      });
      if (candidates.length === 0) {
        return { ok: false, error: "no captured worktree changes after pre-review repair", history, results: implementation.results };
      }
      continue;
    }

    const gateLabel = round === 1 ? "batch-review" : `batch-review-r${round}`;
    phase(
      "Review Gate",
      round === 1
        ? "Review the combined candidate before landing."
        : `Re-review the repaired candidate before landing (round ${round}/${maxReviewRounds}).`
    );
    gate = await reviewWorktreeDiffs(candidates, {
      label: gateLabel,
      context: reviewContext,
      criteria: reviewCriteria,
      reviewers:
        args?.reviewers || [
          {
            label: "correctness",
            runtime: "codex",
            perspective: "Correctness, regression risk, and test evidence.",
          },
          {
            label: "owner-risk",
            runtime: "codex",
            perspective:
              "Owner decision risk after treating run context and task prompts as owner-provided intent; do not ask the owner to reconfirm already stated intent.",
          },
        ],
      maxDiffChars: args?.maxDiffChars || 50000,
    });
    history.push({ step: "review", ...summarizeGate(round, gateLabel, gate) });

    if (gate.applyReady) {
      break;
    }

    if (gate.decision === "needs_owner" || round >= maxReviewRounds) {
      return { ok: false, gate, history, results: implementation.results };
    }

    const repairTasks = tasksForReviewRepair(gate);
    log("repairing tasks=" + repairTasks.map((task) => task.id).join("|"));
    const retainedCandidates =
      repairTasks.length === TASKS.length
        ? []
        : candidates.filter((candidate) => !repairTasks.some((task) => candidateTouchesTask(candidate, task)));
    history.push({
      step: "repair_plan",
      round: round + 1,
      tasks: repairTasks.map((task) => task.id),
      retained_files: candidateFiles(retainedCandidates),
    });
    implementation = await runImplementationRound(round + 1, reviewFeedback(gate), repairTasks);
    candidates = [...retainedCandidates, ...implementation.candidates];
    history.push({
      step: "repair",
      round: round + 1,
      tasks: repairTasks.map((task) => task.id),
      files: candidateFiles(implementation.candidates),
      candidate_files: candidateFiles(candidates),
    });
    if (implementation.candidates.length === 0) {
      return { ok: false, error: "no captured worktree changes after repair", gate, history, results: implementation.results };
    }
  }

  if (!gate?.applyReady) {
    return { ok: false, gate, history, results: implementation.results };
  }

  phase("Land", "Apply approved captured patches atomically.");
  const landed = applyWorktreeDiffs(candidates, { label: "approved-batch" });
  if (!landed.ok) {
    return { ok: false, gate, history, landed };
  }

  phase("Verify", "Verify the landed main working directory.");
  const verifySnapshot = captureMainWorktreeSnapshot({ label: "before-final-verify" });
  const verification = await agent(
    `Read-only verification for the approved batch now landed in the main working directory.

Tasks:
${TASKS.map((task) => `- ${task.id}: ${taskFiles(task).join(", ") || task.file || "(files from prompt)"}`).join("\n")}

Run this command and report the exact result:
${TEST}

Do not modify files, install dependencies, format code, or apply fixes. If verification fails, report the failure and evidence; do not repair it in this step.`,
    {
      id: "verify-landed",
      label: "verify-landed",
      runtime: "codex",
      permission: "limited",
      mockWriteFile: args?.verifyMockWriteFile,
      mockFail: Boolean(args?.verifyMockFail),
    }
  );
  const verifyGuard = assertMainWorktreeUnchanged(verifySnapshot, { label: "final-verify-readonly" });
  const verificationOk = verification?.ok !== false;
  history.push({
    step: "verify",
    ok: verificationOk && verifyGuard.ok,
    guard: {
      ok: verifyGuard.ok,
      files: verifyGuard.files,
      added: verifyGuard.added,
      removed: verifyGuard.removed,
      modified: verifyGuard.modified,
    },
  });

  if (!verifyGuard.ok) {
    const verifyRestore = restoreMainWorktreeSnapshot(verifySnapshot, verifyGuard, { label: "final-verify-restore" });
    history[history.length - 1].restore = {
      ok: verifyRestore.ok,
      restored: verifyRestore.restored,
      removed: verifyRestore.removed,
      errors: verifyRestore.errors,
    };
    return {
      ok: false,
      error: {
        category: "verification_mutated_worktree",
        message: "Final verification changed the main worktree after approve-only landing.",
      },
      gate,
      history,
      landed,
      verification,
      verifyGuard,
      verifyRestore,
    };
  }

  if (!verificationOk) {
    return {
      ok: false,
      error: {
        category: "verification_failed",
        message: "Final verification returned ok:false after approve-only landing.",
      },
      gate,
      history,
      landed,
      verification,
      verifyGuard,
    };
  }

  return {
    ok: true,
    gate,
    history,
    landed,
    verification,
    verifyGuard,
  };
}
