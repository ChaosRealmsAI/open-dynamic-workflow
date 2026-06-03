import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname } from "node:path";
import os from "node:os";
import vm from "node:vm";

const scriptPath = process.env.ODW_SCRIPT_PATH;
const cwd = process.env.ODW_CWD || process.cwd();
const backend = process.env.ODW_BACKEND || "mock";
const runDir = process.env.ODW_RUN_DIR || cwd;
const statePath = process.env.ODW_STATE_PATH || "";
const resumeStatePath = process.env.ODW_RESUME_STATE_PATH || "";
const resumeFrom = process.env.ODW_RESUME_FROM || "";
const codexctlBin = process.env.ODW_CODEXCTL_BIN || "codexctl";
const pandacodeBin = process.env.ODW_PANDACODE_BIN || "pandacode";
const runId = process.env.ODW_RUN_ID || basename(runDir);
const provider = process.env.ODW_PROVIDER || "";
const model = process.env.ODW_MODEL || "";
const effort = process.env.ODW_EFFORT || "low";
const timeout = process.env.ODW_TIMEOUT || "120";
let currentPhase = null;
let workflowPhases = [];
let agentIndex = 0;
let totalAgentsLaunched = 0;
const MAX_TOTAL_AGENTS = 1000;
let worktreeSeq = 0;
let prunedWorktrees = false;
let state = loadState(resumeStatePath || statePath, { strict: Boolean(resumeStatePath || resumeFrom) });
state.agents ??= {};
state.activeAgents = {};
state.failedAgents = {};
state.checkpoints ??= {};

function emit(event) {
  process.stdout.write(JSON.stringify({ ...event, ts: new Date().toISOString() }) + "\n");
}

function loadState(path, { strict = false } = {}) {
  if (!path || !existsSync(path)) {
    return {};
  }
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    // A corrupt RESUME state must fail loudly: silently starting fresh would
    // re-run already-completed executor nodes and reset the budget. A corrupt
    // fresh-run state (rare; we wrote it) falls back to empty.
    if (strict) {
      throw new Error(
        `resume state at ${path} is corrupt and cannot be parsed (${error?.message ?? error}); `
        + "remove it to start fresh or restore a good copy"
      );
    }
    return {};
  }
}

function saveState() {
  if (!statePath) {
    return;
  }
  mkdirSync(dirname(statePath), { recursive: true });
  // Write atomically (tmp + rename) so a crash mid-write can't leave a truncated,
  // unparseable state.json that would poison the next --resume.
  const tmp = `${statePath}.tmp`;
  writeFileSync(tmp, JSON.stringify(state, null, 2));
  renameSync(tmp, statePath);
}

console.log = (...args) => emit({ type: "log", message: args.map(String).join(" ") });
console.error = (...args) => process.stderr.write(args.map(String).join(" ") + "\n");
globalThis.log = (...args) => emit({ type: "log", message: args.map(String).join(" ") });
globalThis.cwd = cwd;
globalThis.odw = { backend, runId, runDir, statePath, resumeFrom };
globalThis.budget = {
  get total() {
    return state.budget?.total ?? null;
  },
  spent() {
    return state.budget?.spent ?? 0;
  },
  remaining() {
    const total = state.budget?.total ?? null;
    return total == null ? Infinity : Math.max(0, total - (state.budget?.spent ?? 0));
  }
};

globalThis.promptSlot = (name, context = {}, suggested = "") => {
  const input = globalThis.args ?? {};
  const prompts = input?.prompts ?? input?.promptSlots ?? {};
  const injected = prompts?.[name];
  if (typeof injected === "string" && injected.trim() !== "") {
    emit({ type: "prompt_slot", name, source: "injected", contextKeys: Object.keys(context ?? {}) });
    return renderPrompt(injected, context);
  }
  if (injected && typeof injected === "object" && typeof injected.template === "string") {
    emit({ type: "prompt_slot", name, source: "injected", contextKeys: Object.keys(context ?? {}) });
    return renderPrompt(injected.template, { ...context, ...(injected.context ?? {}) });
  }
  const allowSuggested = backend === "mock"
    || input?.allowSuggestedPrompts === true
    || input?.useSuggestedPrompts === true;
  if (allowSuggested && typeof suggested === "string" && suggested.trim() !== "") {
    emit({ type: "prompt_slot", name, source: "suggested", contextKeys: Object.keys(context ?? {}) });
    return renderPrompt(suggested, context);
  }
  throw new Error(`missing prompt slot ${name}; pass input.prompts.${name}`);
};

function renderPrompt(template, context) {
  return String(template)
    .replaceAll("{{context}}", JSON.stringify(context ?? {}, null, 2))
    .replaceAll("{{input}}", JSON.stringify(globalThis.args ?? null, null, 2));
}

// Per-phase model: meta.phases[].model lets a phase declare a default model that
// its agents inherit when they do not set options.model (matches built-in Workflow).
function phaseModelFor(title) {
  if (!title || !Array.isArray(workflowPhases)) {
    return null;
  }
  const entry = workflowPhases.find((p) => p && p.title === title);
  return entry?.model ?? null;
}

globalThis.phase = (title, detail = "") => {
  currentPhase = title;
  emit({ type: "phase", title, detail });
};

globalThis.checkpoint = (name, value = null) => {
  state.checkpoints[name] = {
    name,
    value,
    ts: new Date().toISOString()
  };
  saveState();
  emit({ type: "checkpoint", name });
};

globalThis.agent = async (prompt, options = {}) => {
  agentIndex += 1;
  const label = options.label || `agent-${agentIndex}`;
  const phase = options.phase || currentPhase || "";
  const agentType = firstText(options.agentType, options.nodeType, options.role) || undefined;
  const normalizedOptions = { ...options, label, phase };
  if (agentType) {
    normalizedOptions.agentType = agentType;
  }
  if (!normalizedOptions.model) {
    const phaseModel = phaseModelFor(phase);
    if (phaseModel) {
      normalizedOptions.model = phaseModel;
    }
  }
  const key = options.id || options.nodeId || agentCacheKey(prompt, normalizedOptions);
  // Content fingerprint of (prompt + options), kept even when an explicit id keys
  // the cache. On resume a cached node is only skipped when its fingerprint still
  // matches — so editing a node's prompt re-runs it instead of returning a stale
  // result. (undefined fingerprint = pre-fingerprint cache entry; stay compatible.)
  const fingerprint = agentCacheKey(prompt, normalizedOptions);
  const schema = loadSchemaDescriptor(options.schema);
  const schemaDescription = resolveSchemaDescription(normalizedOptions, schema);
  const maxAttempts = Math.max(1, Number(options.retry?.maxAttempts || options.maxAttempts || 1));
  // An unloadable schema (typo'd path, or a non-object/non-string value) is a
  // workflow config error, not a transient mismatch. Fail fast and NON-retryably
  // with a clear message instead of wasting `maxAttempts` re-running the node and
  // returning a confusing schema_mismatch envelope (the file won't appear on retry).
  if (schema && schema.error) {
    const result = {
      ok: false,
      error: {
        category: "schema_load_error",
        message: `agent node "${label}" schema "${schema.name}" could not be loaded: ${schema.error}`,
        retryable: false
      }
    };
    markAgentFailed({ key, label, phase, agentType, attempt: 1, maxAttempts, result });
    emit({ type: "agent_done", index: agentIndex, key, label, phase, agentType, attempt: 1, maxAttempts, ok: false, result });
    return result;
  }
  const cached = state.agents[key];
  if (
    cached?.ok !== false
    && cached?.result !== undefined
    && (cached.fingerprint === undefined || cached.fingerprint === fingerprint)
  ) {
    emit({ type: "agent_skip", index: agentIndex, key, label, phase, agentType, reason: "cached", result: cached.result });
    return cached.result;
  }

  if (state.budget?.total != null && state.budget.spent >= state.budget.total) {
    throw new Error(`workflow budget exhausted: spent ${state.budget.spent} >= total ${state.budget.total} tokens`);
  }

  totalAgentsLaunched += 1;
  if (totalAgentsLaunched > MAX_TOTAL_AGENTS) {
    throw new Error(`workflow exceeded the ${MAX_TOTAL_AGENTS}-agent runaway backstop`);
  }

  let attemptPrompt = appendSchemaContract(prompt, schema, schemaDescription);
  let previousResult = null;
  let previousFailure = null;
  // The node's literal config (from the agent() call) + the prompt, so a graph
  // report can show exactly what the code declares — no editorialising.
  const displayRuntime = inferPandaRuntime(normalizedOptions);
  const displayModel = normalizedOptions.model || model || "inherit";
  const promptPreview = String(prompt).slice(0, 8000);
  const nodeConfig = {
    runtime: displayRuntime,
    model: displayModel,
    provider: options.provider || options.bambooProvider || undefined,
    schema: schemaNameOf(options.schema) || undefined,
    isolation: options.isolation || undefined,
    agentType: agentType || undefined,
    effort: options.effort || undefined,
    timeout: options.timeout ?? options.timeoutMs ?? undefined,
    maxAttempts: maxAttempts > 1 ? maxAttempts : undefined,
  };
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    markAgentActive({ key, label, phase, agentType, attempt, maxAttempts });
    emit({ type: "agent_start", index: agentIndex, key, label, phase, agentType, runtime: displayRuntime, model: displayModel, promptPreview, config: nodeConfig, attempt, maxAttempts });
    try {
      const rawResult = await runAgent(attemptPrompt, { ...normalizedOptions, attempt, previousFailure });
      // Count every dispatched attempt's tokens. A node that retries or ultimately
      // fails still burned tokens, so accruing only on success undercounts the
      // budget and lets budget-bounded loops overrun.
      accrueBudget(rawResult);
      const result = normalizeNodeResult(rawResult, normalizedOptions, schema);
      if (rawResult && rawResult.__worktree && result && typeof result === "object" && !Array.isArray(result)) {
        result.worktree = rawResult.__worktree;
        delete result.__worktree;
      }
      const validation = validateNodeResult(result, schema);
      const errorFeedbackValidation = result?.ok === false
        ? validateNodeResult(result, loadSchemaDescriptor(".odw/schemas/error-feedback.schema.json"))
        : null;
      const structuredFailure = Boolean(result?.ok === false && errorFeedbackValidation?.valid);
      const ok = result?.ok !== false && validation.valid;
      if (ok) {
        // Built-in parity: with no schema, return the executor's final text (or a
        // lean {text, worktree} when a worktree captured changes) instead of the
        // verbose raw report. Schema'd nodes keep their validated object.
        const finalResult = schema ? result : leanAgentResult(result);
        // Backfill the real model the executor resolved when the script left it
        // implicit, so observability shows what ran instead of "inherit".
        const resolvedModel = displayModel === "inherit"
          ? (resolvedModelFromReport(rawResult) || displayModel)
          : displayModel;
        state.agents[key] = {
          ok,
          index: agentIndex,
          key,
          fingerprint,
          label,
          phase,
          agentType,
          attempt,
          maxAttempts,
          schema: schema?.name || null,
          model: resolvedModel,
          result: finalResult,
          tokens: nodeTotalTokens(rawResult),
          ts: new Date().toISOString()
        };
        delete state.activeAgents[key];
        delete state.failedAgents[key];
        saveState();
        emit({ type: "agent_done", index: agentIndex, key, label, phase, agentType, runtime: displayRuntime, model: resolvedModel, attempt, maxAttempts, ok, tokens: nodeTotalTokens(rawResult), result: finalResult });
        return finalResult;
      }

      previousResult = result;
      if (!validation.valid && !structuredFailure) {
        previousFailure = schemaMismatchResult({
          key,
          label,
          phase,
          agentType,
          attempt,
          maxAttempts,
          schema,
          validation,
          result
        });
        const retryable = attempt < maxAttempts;
        emit({
          type: "agent_schema_invalid",
          index: agentIndex,
          key,
          label,
          phase,
          agentType,
          attempt,
          maxAttempts,
          schema: schema?.name || null,
          retryable,
          issues: validation.issues,
          result
        });
        if (retryable) {
          attemptPrompt = retryPrompt(prompt, previousFailure, schema, schemaDescription);
          state.activeAgents[key] = {
            ...state.activeAgents[key],
            state: "retrying",
            retryReason: "schema_mismatch",
            nextAttempt: attempt + 1,
            updatedAt: new Date().toISOString()
          };
          saveState();
          emit({ type: "agent_retry", index: agentIndex, key, label, phase, agentType, attempt, nextAttempt: attempt + 1, maxAttempts, reason: "schema_mismatch" });
          continue;
        }
        markAgentFailed({ key, label, phase, agentType, attempt, maxAttempts, result: previousFailure });
        emit({ type: "agent_done", index: agentIndex, key, label, phase, agentType, attempt, maxAttempts, ok: false, result: previousFailure });
        return previousFailure;
      }

      const retryable = Boolean(result?.feedback?.retryable ?? result?.error?.retryable);
      if (retryable && attempt < maxAttempts) {
        previousFailure = retryableFailureResult({
          key,
          label,
          phase,
          agentType,
          attempt,
          maxAttempts,
          result
        });
        attemptPrompt = retryPrompt(prompt, previousFailure, schema, schemaDescription);
        state.activeAgents[key] = {
          ...state.activeAgents[key],
          state: "retrying",
          retryReason: result?.error?.category || "worker_failed",
          nextAttempt: attempt + 1,
          updatedAt: new Date().toISOString()
        };
        saveState();
        emit({ type: "agent_retry", index: agentIndex, key, label, phase, agentType, attempt, nextAttempt: attempt + 1, maxAttempts, reason: result?.error?.category || "worker_failed" });
        continue;
      }

      markAgentFailed({ key, label, phase, agentType, attempt, maxAttempts, result });
      emit({ type: "agent_done", index: agentIndex, key, label, phase, agentType, attempt, maxAttempts, ok: false, result });
      return result;
    } catch (error) {
      previousResult = {
        ok: false,
        error: {
          category: "workflow_agent_failed",
          message: String(error?.message ?? error)
        }
      };
      const retryable = attempt < maxAttempts;
      previousFailure = retryableFailureResult({
        key,
        label,
        phase,
        agentType,
        attempt,
        maxAttempts,
        result: previousResult
      });
      if (retryable) {
        attemptPrompt = retryPrompt(prompt, previousFailure, schema, schemaDescription);
        state.activeAgents[key] = {
          ...state.activeAgents[key],
          state: "retrying",
          retryReason: "workflow_agent_failed",
          nextAttempt: attempt + 1,
          updatedAt: new Date().toISOString()
        };
        saveState();
        emit({ type: "agent_retry", index: agentIndex, key, label, phase, agentType, attempt, nextAttempt: attempt + 1, maxAttempts, reason: "workflow_agent_failed" });
        continue;
      }
      markAgentFailed({ key, label, phase, agentType, attempt, maxAttempts, result: previousFailure });
      emit({ type: "agent_done", index: agentIndex, key, label, phase, agentType, attempt, maxAttempts, ok: false, result: previousFailure });
      return previousFailure;
    }
  }

  return previousResult ?? {
    ok: false,
    error: { category: "workflow_agent_failed", message: "agent exited without a result" }
  };
};

function markAgentActive({ key, label, phase, agentType, attempt, maxAttempts }) {
  state.activeAgents[key] = {
    key,
    index: agentIndex,
    label,
    phase,
    agentType,
    attempt,
    maxAttempts,
    state: "running",
    startedAt: state.activeAgents[key]?.startedAt || new Date().toISOString(),
    updatedAt: new Date().toISOString()
  };
  saveState();
}

function markAgentFailed({ key, label, phase, agentType, attempt, maxAttempts, result }) {
  delete state.activeAgents[key];
  state.failedAgents[key] = {
    key,
    index: agentIndex,
    label,
    phase,
    agentType,
    attempt,
    maxAttempts,
    ok: false,
    result,
    ts: new Date().toISOString()
  };
  saveState();
}

function agentCacheKey(prompt, options) {
  return `agent:${hashString(stableStringify({ prompt: String(prompt), options }))}`;
}

function stableStringify(value) {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(",")}]`;
  }
  const entries = Object.keys(value)
    .filter((key) => typeof value[key] !== "function" && value[key] !== undefined)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`);
  return `{${entries.join(",")}}`;
}

function hashString(value) {
  let hash = 2166136261;
  for (let i = 0; i < value.length; i += 1) {
    hash ^= value.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0).toString(36);
}

function loadSchemaDescriptor(schemaSpec) {
  if (!schemaSpec) {
    return null;
  }
  if (typeof schemaSpec === "object") {
    return {
      name: schemaSpec.title || "inline schema",
      schema: schemaSpec
    };
  }
  if (typeof schemaSpec !== "string") {
    return {
      name: "invalid schema",
      error: `schema must be a path or object, got ${typeof schemaSpec}`
    };
  }
  const schemaPath = schemaSpec.startsWith("/") ? schemaSpec : `${cwd}/${schemaSpec}`;
  try {
    return {
      name: schemaSpec,
      path: schemaPath,
      schema: JSON.parse(readFileSync(schemaPath, "utf8"))
    };
  } catch (error) {
    return {
      name: schemaSpec,
      path: schemaPath,
      error: String(error?.message ?? error)
    };
  }
}

// schemaDescription is OPTIONAL (matches built-in Workflow). When provided it is
// added to the final-response contract; when omitted the schema is still enforced.
function resolveSchemaDescription(options, descriptor) {
  if (!descriptor || descriptor.error || !descriptor.schema) {
    return "";
  }
  return firstText(
    options.schemaDescription,
    options.outputDescription,
    options.finalResponseDescription
  );
}

function appendSchemaContract(prompt, descriptor, schemaDescription = "") {
  if (!descriptor || descriptor.error || !descriptor.schema) {
    return String(prompt);
  }
  const lines = [
    String(prompt),
    "",
    "ODW final response contract:",
    "First complete this node task normally, including any file edits, commands, analysis, or checks required by the prompt.",
    "The JSON Schema below constrains only your final assistant response for workflow orchestration. It does not constrain intermediate tool calls, file edits, commands, or internal reasoning."
  ];
  if (schemaDescription) {
    lines.push(`Final response purpose: ${schemaDescription}`);
  }
  lines.push(
    `When the task is complete, make your final assistant response exactly one JSON object that satisfies ${descriptor.name}.`,
    "Do not wrap the final JSON in markdown fences. Do not add prose before or after the final JSON object.",
    "If you attempted the task but cannot complete it, make the final assistant response an object matching .odw/schemas/error-feedback.schema.json instead of prose.",
    "Required JSON Schema:",
    JSON.stringify(descriptor.schema, null, 2)
  );
  return lines.join("\n");
}

function validateNodeResult(result, descriptor) {
  if (!descriptor) {
    return { valid: true, schema: null, issues: [] };
  }
  if (descriptor.error) {
    return {
      valid: false,
      schema: descriptor.name,
      issues: [`schema ${descriptor.name} could not be loaded: ${descriptor.error}`]
    };
  }
  const issues = [];
  validateAgainstSchema(result, descriptor.schema, "$", issues);
  return {
    valid: issues.length === 0,
    schema: descriptor.name,
    issues: issues.slice(0, 40)
  };
}

function validateAgainstSchema(value, schema, path, issues) {
  if (!schema || typeof schema !== "object") {
    return;
  }
  if (Array.isArray(schema.oneOf)) {
    const matched = schema.oneOf.some((candidate) => {
      const nested = [];
      validateAgainstSchema(value, candidate, path, nested);
      return nested.length === 0;
    });
    if (!matched) {
      issues.push(`${path} must match one of ${schema.oneOf.length} schema variants`);
    }
    // Do NOT return: JSON Schema applies oneOf AND any sibling keywords (type,
    // required, properties, …). Falling through checks those too.
  }
  if (schema.const !== undefined && stableStringify(value) !== stableStringify(schema.const)) {
    issues.push(`${path} must equal ${JSON.stringify(schema.const)}`);
  }
  if (Array.isArray(schema.enum) && !schema.enum.some((item) => stableStringify(item) === stableStringify(value))) {
    issues.push(`${path} must be one of ${schema.enum.map((item) => JSON.stringify(item)).join(", ")}`);
  }
  if (schema.type !== undefined && !schemaTypeMatches(value, schema.type)) {
    issues.push(`${path} must be ${Array.isArray(schema.type) ? schema.type.join(" or ") : schema.type}`);
    return;
  }
  if (typeof schema.minimum === "number" && typeof value === "number" && value < schema.minimum) {
    issues.push(`${path} must be >= ${schema.minimum}`);
  }
  if (Array.isArray(schema.required) && value && typeof value === "object" && !Array.isArray(value)) {
    for (const key of schema.required) {
      if (value[key] === undefined) {
        issues.push(`${path}.${key} is required`);
      }
    }
  }
  if (schema.properties && value && typeof value === "object" && !Array.isArray(value)) {
    for (const [key, propertySchema] of Object.entries(schema.properties)) {
      if (value[key] !== undefined) {
        validateAgainstSchema(value[key], propertySchema, `${path}.${key}`, issues);
      }
    }
  }
  if (schema.items && Array.isArray(value)) {
    value.forEach((item, index) => validateAgainstSchema(item, schema.items, `${path}[${index}]`, issues));
  }
}

function schemaTypeMatches(value, schemaType) {
  if (Array.isArray(schemaType)) {
    return schemaType.some((candidate) => schemaTypeMatches(value, candidate));
  }
  if (schemaType === "array") {
    return Array.isArray(value);
  }
  if (schemaType === "object") {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }
  if (schemaType === "integer") {
    return Number.isInteger(value);
  }
  if (schemaType === "null") {
    return value === null;
  }
  return typeof value === schemaType;
}

function normalizeNodeResult(result, options, schemaDescriptor = null) {
  if (!result || typeof result !== "object" || Array.isArray(result)) {
    return result;
  }
  const schemaName = schemaNameOf(options.schema);
  if (
    schemaName
    && !schemaName.endsWith("codex-plan.schema.json")
    && !schemaName.endsWith("codex-result.schema.json")
  ) {
    const structured = extractStructuredCodexOutput(result, schemaDescriptor);
    if (structured && typeof structured === "object" && !Array.isArray(structured)) {
      return structured;
    }
  }
  if (schemaName.endsWith("codex-plan.schema.json")) {
    return normalizeCodexPlanResult(result);
  }
  if (schemaName.endsWith("codex-result.schema.json")) {
    return normalizeCodexImplementationResult(result);
  }
  return result;
}

function extractStructuredCodexOutput(report, schemaDescriptor = null) {
  const codex = report?.codex && typeof report.codex === "object" ? report.codex : null;
  const summary = report?.summary && typeof report.summary === "object" ? report.summary : null;
  for (const candidate of [
    report?.structured_output,
    report?.structuredOutput,
    report?.result,
    report?.output,
    report?.json,
    summary?.structured_output,
    summary?.structuredOutput,
    summary?.result,
    summary?.output,
    summary?.json,
    codex?.structured_output,
    codex?.structuredOutput,
    codex?.result,
    codex?.output,
    codex?.json
  ]) {
    if (candidate && typeof candidate === "object" && !Array.isArray(candidate)) {
      return candidate;
    }
  }
  if (codex && !looksLikeCodexEnvelope(codex)) {
    return codex;
  }
  for (const message of report?.agent_messages ?? []) {
    const parsed = parseJsonObjectFromText(message, schemaDescriptor);
    if (parsed) {
      return parsed;
    }
  }
  for (const message of codex?.agent_messages ?? []) {
    const parsed = parseJsonObjectFromText(message, schemaDescriptor);
    if (parsed) {
      return parsed;
    }
  }
  for (const plan of codex?.plans ?? []) {
    const parsed = parseJsonObjectFromText(plan, schemaDescriptor);
    if (parsed) {
      return parsed;
    }
  }
  for (const item of codex?.items ?? []) {
    const parsed = parseJsonObjectFromText(item?.aggregatedOutput ?? item?.text, schemaDescriptor);
    if (parsed) {
      return parsed;
    }
  }
  return parseJsonObjectFromText(firstText(
    report?.last_agent_message,
    report?.lastAgentMessage,
    summary?.last_agent_message,
    summary?.lastAgentMessage,
    summary?.message,
    summary?.text,
    summary?.capture_tail,
    report?.wait?.capture_tail,
    report?.capture_tail,
    codex?.last_agent_message,
    codex?.lastAgentMessage,
    codex?.message,
    codex?.text,
    report?.stdout,
    report?.stdout_tail
  ), schemaDescriptor);
}

function looksLikeCodexEnvelope(value) {
  return Boolean(
    value?.run_id
    || value?.runId
    || value?.thread_id
    || value?.threadId
    || value?.last_agent_message
    || value?.lastAgentMessage
    || value?.plans
    || value?.events
  );
}

function parseJsonObjectFromText(text, schemaDescriptor = null) {
  const raw = String(text || "").trim();
  if (!raw) {
    return null;
  }
  const fenced = raw.match(/```(?:json)?\s*([\s\S]*?)```/i);
  const parsedObjects = [];
  for (const candidate of [
    raw,
    fenced?.[1],
    raw.slice(raw.indexOf("{"), raw.lastIndexOf("}") + 1),
    ...extractJsonObjectStrings(raw)
  ]) {
    if (!candidate || !candidate.trim().startsWith("{")) {
      continue;
    }
    try {
      const parsed = JSON.parse(candidate.trim());
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        parsedObjects.push(parsed);
      }
    } catch {
      // Try the next candidate.
    }
  }
  if (schemaDescriptor?.schema) {
    for (const parsed of parsedObjects) {
      if (validateNodeResult(parsed, schemaDescriptor).valid) {
        return parsed;
      }
    }
    return null;
  }
  return parsedObjects[0] || null;
}

function extractJsonObjectStrings(text) {
  const raw = String(text || "");
  const objects = [];
  let depth = 0;
  let start = -1;
  let inString = false;
  let escaped = false;
  for (let index = 0; index < raw.length; index += 1) {
    const ch = raw[index];
    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === "\"") {
        inString = false;
      }
      continue;
    }
    if (ch === "\"") {
      inString = true;
      continue;
    }
    if (ch === "{") {
      if (depth === 0) {
        start = index;
      }
      depth += 1;
      continue;
    }
    if (ch === "}" && depth > 0) {
      depth -= 1;
      if (depth === 0 && start >= 0) {
        objects.push(raw.slice(start, index + 1));
        start = -1;
      }
    }
  }
  return objects;
}

function normalizeCodexPlanResult(report) {
  const result = { ...report };
  const codex = result.codex && typeof result.codex === "object" ? result.codex : {};
  // pandacode reports carry the lifecycle as `state` (incl. "waiting_for_user");
  // include it so a needs-input turn isn't misread as a completed plan.
  const rawState = result.status || result.state || codex.status || codex.state || (result.ok === false ? "failed" : "completed");
  const sourceStatus = rawState === "waiting_for_user" ? "needs_input" : rawState;
  result.status = sourceStatus === "needs_input" ? "needs_input" : sourceStatus === "failed" || result.ok === false ? "failed" : "planned";
  result.run_id = result.run_id || codex.run_id || result.runId;
  result.thread_id = result.thread_id || codex.thread_id || codex.threadId || result.threadId;
  if (!result.adapter || typeof result.adapter !== "object") {
    result.adapter = {
      backend: result.backend || codex.backend || "pandacode",
      mode: result.action === "start" ? "session_start" : "plan",
      command: result.shell || "",
      stdout_tail: result.stdout_tail || "",
      stderr_tail: result.stderr_tail || "",
      session_socket: result.session_socket || codex.session_socket,
      log_dir: result.log_dir || codex.log_dir
    };
  }
  if (!result.plan || typeof result.plan !== "object") {
    const text = firstText(codex.plans?.[0], codex.last_agent_message, result.last_agent_message, result.stdout_tail);
    result.plan = {
      summary: firstNonEmptyLine(text) || "Codex returned a plan.",
      steps: extractListLines(text),
      files: extractLikelyFiles(text)
    };
  }
  if (!Array.isArray(result.constraints)) {
    result.constraints = [];
  }
  if (!Array.isArray(result.verification)) {
    result.verification = [];
  }
  if (!Array.isArray(result.questions)) {
    result.questions = Array.isArray(codex.questions) ? codex.questions : [];
  }
  if (!Array.isArray(result.risks)) {
    result.risks = [];
  }
  if (result.error === undefined) {
    result.error = null;
  }
  return result;
}

function normalizeCodexImplementationResult(report) {
  const result = { ...report };
  const codex = result.codex && typeof result.codex === "object" ? result.codex : {};
  result.run_id = result.run_id || codex.run_id || result.runId || "";
  result.thread_id = result.thread_id || codex.thread_id || codex.threadId || result.threadId;
  // Include pandacode's `state` field (incl. "waiting_for_user" -> needs_input)
  // so a needs-input codex turn isn't misreported as a completed implementation.
  const rawState = result.status || result.state || codex.status || codex.state || (result.ok === false ? "failed" : "completed");
  const sourceStatus = rawState === "waiting_for_user" ? "needs_input" : rawState;
  result.status = ["completed", "failed", "needs_input", "stopped"].includes(sourceStatus)
    ? sourceStatus
    : result.ok === false
      ? "failed"
      : "completed";
  if (!result.adapter || typeof result.adapter !== "object") {
    result.adapter = {
      backend: result.backend || codex.backend || "pandacode",
      runtime: result.runtime || codex.runtime,
      start_command: result.shell || "",
      read_command: result.action === "read" ? result.shell || "" : "",
      stdout_tail: result.stdout_tail || "",
      stderr_tail: result.stderr_tail || "",
      session_socket: result.session_socket || codex.session_socket,
      log_dir: result.log_dir || codex.log_dir
    };
  }
  if (!Array.isArray(result.changed_files)) {
    result.changed_files = extractLikelyFiles(firstText(codex.last_agent_message, result.last_agent_message, result.stdout_tail));
  }
  if (!Array.isArray(result.verification)) {
    result.verification = extractCommandVerification(codex);
  }
  if (!Array.isArray(result.risks)) {
    result.risks = [];
  }
  if (result.error === undefined) {
    result.error = null;
  }
  return result;
}

function schemaNameOf(schemaSpec) {
  if (typeof schemaSpec === "string") {
    return schemaSpec;
  }
  if (schemaSpec && typeof schemaSpec === "object") {
    return schemaSpec.title || "inline schema";
  }
  return "";
}

function firstText(...values) {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) {
      return value;
    }
  }
  return "";
}

// The executor's final assistant message, dug out of a pandacode/codex report
// envelope. Used to collapse a no-schema node result to plain text (built-in
// Workflow parity: `agent(prompt)` without a schema returns the final text).
function finalAgentText(report) {
  const codex = report?.codex && typeof report.codex === "object" ? report.codex : {};
  const summary = report?.summary && typeof report.summary === "object" ? report.summary : {};
  return firstText(
    report?.last_agent_message,
    report?.lastAgentMessage,
    summary.last_agent_message,
    summary.lastAgentMessage,
    summary.message,
    summary.text,
    report?.message,
    report?.text,
    codex.last_agent_message,
    codex.lastAgentMessage,
    codex.message,
    codex.text,
    report?.stdout_tail,
    report?.adapter?.stdout_tail
  );
}

// True when `result` is a real-executor (pandacode) report envelope rather than
// a caller/mock/structured object — so only those get collapsed to lean form.
function isExecutorReport(result) {
  if (!result || typeof result !== "object" || Array.isArray(result)) {
    return false;
  }
  return result.backend === "pandacode"
    || result.adapter?.backend === "pandacode"
    || (typeof result.runtime === "string"
      && (result.last_agent_message !== undefined
        || result.lastAgentMessage !== undefined
        || result.summary !== undefined
        || result.thread_id !== undefined));
}

// Built-in Workflow parity for the no-schema path: a successful node returns the
// executor's final text (string), NOT the giant raw report. When the node ran in
// a worktree with captured changes, return a lean `{ text, worktree }` so the
// diff survives without the socket/thread/log-path noise. Schema'd nodes keep
// their validated structured object (handled by the caller before this).
function leanAgentResult(result) {
  if (!isExecutorReport(result)) {
    return result;
  }
  const text = stripCompletionMarker(finalAgentText(result));
  const worktree = result.worktree;
  // If the node ran in a worktree, always return {text, worktree} — even when it
  // made no changes — so worktree nodes have a consistent shape and a caller can
  // reliably check `result.worktree.changed`. Previously an unchanged worktree
  // node collapsed to a bare string (worktree dropped), so `result.worktree` was
  // undefined and indistinguishable from a non-worktree node.
  if (worktree && typeof worktree === "object") {
    return { text, worktree };
  }
  return text;
}

function firstNonEmptyLine(text) {
  return String(text)
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean) || "";
}

function extractListLines(text) {
  const lines = String(text)
    .split(/\r?\n/)
    .map((line) => line.trim().replace(/^[-*]\s+/, "").replace(/^\d+\.\s+/, ""))
    .filter(Boolean);
  return lines.slice(0, 12);
}

function extractLikelyFiles(text) {
  const matches = String(text).match(/[A-Za-z0-9_./-]+\.[A-Za-z0-9_/-]+/g) || [];
  return Array.from(new Set(matches.filter((item) => !item.startsWith("http")).slice(0, 32)));
}

function extractCommandVerification(codex) {
  const items = Array.isArray(codex.items) ? codex.items : [];
  const commands = [];
  for (const item of items) {
    if (item?.type === "commandExecution" && typeof item.command === "string") {
      commands.push({
        command: item.command,
        status: item.exitCode === 0 ? "passed" : "failed",
        output_tail: String(item.aggregatedOutput || "").slice(-4000)
      });
    }
  }
  return commands;
}

function schemaMismatchResult({ key, label, phase, agentType, attempt, maxAttempts, schema, validation, result }) {
  const retryable = attempt < maxAttempts;
  return {
    ok: false,
    origin: { phase, agent: label, agentType, backend, attempt, node_key: key, schema: schema?.name || null },
    error: {
      category: "schema_mismatch",
      message: `Node output did not match ${schema?.name || "the requested schema"}`,
      issues: validation.issues
    },
    feedback: {
      retryable,
      user_message: "The worker returned output that did not match the required schema.",
      next_action: retryable ? "Retry the same node with schema errors injected into context." : "Route this failure to a feedback or terminal node.",
      retry_prompt: retryable ? `Fix the output to satisfy ${schema?.name || "the requested schema"}.` : undefined
    },
    context: {
      node: { key, label, phase, agentType },
      attempt,
      maxAttempts,
      schema: schema?.name || null,
      validation,
      previous_result: result
    }
  };
}

function retryableFailureResult({ key, label, phase, agentType, attempt, maxAttempts, result }) {
  const retryable = attempt < maxAttempts;
  return {
    ok: false,
    origin: { phase, agent: label, agentType, backend, attempt, node_key: key },
    error: {
      category: result?.error?.category || "workflow_agent_failed",
      message: result?.error?.message || "Worker failed before returning a successful result"
    },
    feedback: {
      retryable,
      user_message: result?.feedback?.user_message || result?.error?.message || "Worker failed.",
      next_action: retryable ? "Retry the same node with failure context injected." : "Route this failure to a feedback or terminal node.",
      retry_prompt: result?.feedback?.retry_prompt || result?.error?.retry_prompt
    },
    context: {
      node: { key, label, phase, agentType },
      attempt,
      maxAttempts,
      previous_result: result
    }
  };
}

function retryPrompt(originalPrompt, failure, schema = null, schemaDescription = "") {
  const issues = failure?.context?.validation?.issues || [];
  return appendSchemaContract([
    String(originalPrompt),
    "",
    "ODW retry context:",
    `Previous attempt: ${failure?.context?.attempt || 1}/${failure?.context?.maxAttempts || 1}`,
    `Failure category: ${failure?.error?.category || "unknown"}`,
    `Failure message: ${failure?.error?.message || ""}`,
    issues.length ? `Schema issues:\n${issues.map((issue) => `- ${issue}`).join("\n")}` : "",
    "Previous result:",
    truncateJson(failure?.context?.previous_result, 6000),
    "",
    "Retry instruction:",
    "Do the same node task again. Preserve the original intent, fix only the failed contract, and return output that satisfies the requested schema."
  ].filter(Boolean).join("\n"), schema, schemaDescription);
}

function truncateJson(value, limit) {
  const text = JSON.stringify(value, null, 2);
  return text.length <= limit ? text : `${text.slice(0, limit)}\n...<truncated>`;
}

const CONCURRENCY_HARD_CAP = 16;
let cachedMaxConcurrency = null;
function getMaxConcurrency() {
  if (cachedMaxConcurrency === null) {
    let cores = CONCURRENCY_HARD_CAP;
    try {
      cores = os.cpus().length;
    } catch {
      cores = CONCURRENCY_HARD_CAP;
    }
    cachedMaxConcurrency = Math.max(1, Math.min(CONCURRENCY_HARD_CAP, cores - 2));
  }
  return cachedMaxConcurrency;
}

async function runConcurrent(items, max, runItem, onItemError) {
  const results = new Array(items.length);
  let next = 0;

  async function worker() {
    while (true) {
      const index = next;
      if (index >= items.length) {
        return;
      }
      next += 1;
      try {
        results[index] = await runItem(items[index], index);
      } catch (error) {
        // Match built-in Workflow: a thrown item resolves to null and never
        // rejects the whole batch; the others still run. Callers .filter(Boolean).
        results[index] = null;
        if (onItemError) {
          onItemError(error, index);
        }
      }
    }
  }

  await Promise.all(Array.from({ length: max }, () => worker()));
  return results;
}

globalThis.parallel = async (nodes, options = {}) => {
  if (!Array.isArray(nodes)) {
    throw new Error("parallel(thunks) requires an array");
  }
  const phase = options.phase || currentPhase || "";
  const label = options.label || options.id || "parallel";
  const requestedMax = Number(options.max || options.concurrency || getMaxConcurrency());
  const max = nodes.length === 0 ? 0 : Math.max(1, Math.min(requestedMax, getMaxConcurrency(), nodes.length));
  emit({ type: "parallel_start", label, phase, count: nodes.length, max });
  let results = [];
  let ok = false;
  try {
    results = await runConcurrent(
      nodes,
      max,
      (node, index) => runParallelNode(node, index, phase),
      (error, index) => emit({ type: "parallel_item_error", label, phase, index, message: String(error?.message ?? error) })
    );
    // A null slot means that thunk threw (runConcurrent maps errors to null);
    // count it as not-ok so the *_done telemetry reflects real failures.
    ok = results.every((result) => result !== null && result?.ok !== false);
    return results;
  } finally {
    emit({ type: "parallel_done", label, phase, count: nodes.length, max, ok });
  }
};

async function runParallelNode(node, index, phase) {
  if (typeof node === "function") {
    return node(index);
  }
  if (!node || typeof node !== "object") {
    throw new Error(`parallel node ${index} must be an object or function`);
  }
  const { prompt, input, ...options } = node;
  return globalThis.agent(prompt ?? input ?? "", {
    ...options,
    id: options.id || options.nodeId || `parallel-${index}`,
    label: options.label || `parallel-${index + 1}`,
    phase: options.phase || phase
  });
}

globalThis.fanout = async (items, mapper, options = {}) => {
  if (!Array.isArray(items)) {
    throw new Error("fanout(items, mapper) requires an item array");
  }
  if (typeof mapper !== "function") {
    throw new Error("fanout mapper must be a function");
  }
  return globalThis.parallel(
    items.map((item, index) => () => mapper(item, index)),
    { ...options, label: options.label || options.id || "fanout" }
  );
};

globalThis.pipeline = async (items, ...stages) => {
  if (!Array.isArray(items)) {
    throw new Error("pipeline(items, ...stages) requires an item array");
  }
  for (const stage of stages) {
    if (typeof stage !== "function") {
      throw new Error("pipeline stages must be functions");
    }
  }
  const phase = currentPhase || "";
  const label = "pipeline";
  const max = items.length === 0 ? 0 : Math.min(getMaxConcurrency(), items.length);
  emit({ type: "pipeline_start", label, phase, count: items.length, stages: stages.length, max });
  let results = [];
  let ok = false;
  try {
    results = await runConcurrent(
      items,
      max,
      async (item, index) => {
        let value = item;
        for (const stage of stages) {
          value = await stage(value, item, index);
        }
        return value;
      },
      (error, index) => emit({ type: "pipeline_item_error", label, phase, index, message: String(error?.message ?? error) })
    );
    // A null slot means that thunk threw (runConcurrent maps errors to null);
    // count it as not-ok so the *_done telemetry reflects real failures.
    ok = results.every((result) => result !== null && result?.ok !== false);
    return results;
  } finally {
    emit({ type: "pipeline_done", label, phase, count: items.length, stages: stages.length, max, ok });
  }
};

let inNestedWorkflow = false;
function resolveWorkflowPath(nameOrRef) {
  const ref = String(nameOrRef ?? "").trim();
  if (!ref) {
    throw new Error("workflow(nameOrRef) requires a workflow name or path");
  }
  const candidates = ref.endsWith(".js") || ref.includes("/") || ref.startsWith(".")
    ? [ref.startsWith("/") ? ref : `${cwd}/${ref}`]
    : [
        `${cwd}/.claude/workflows/${ref}.js`,
        `${cwd}/.claude/workflows/odw-${ref}.js`
      ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  throw new Error(`workflow "${ref}" not found (looked in: ${candidates.join(", ")})`);
}

// Run a saved/sibling workflow inline as one step. Shares this run's agent
// counter, concurrency caps, 1000-agent backstop, budget, and state. 1 level
// only: a sub-workflow that itself calls workflow() throws.
globalThis.workflow = async (nameOrRef, childArgs = {}) => {
  if (inNestedWorkflow) {
    throw new Error("nested workflow() is 1-level only; a sub-workflow may not call workflow()");
  }
  const path = resolveWorkflowPath(nameOrRef);
  const child = prepareWorkflowModule(path, childArgs);
  if (typeof child.workflow !== "function") {
    throw new Error(`sub-workflow ${path} does not export a runnable workflow`);
  }
  const childName = child.meta?.name || basename(path);
  emit({ type: "workflow_call_start", name: childName, path });
  const savedPhase = currentPhase;
  inNestedWorkflow = true;
  try {
    const result = await child.workflow(childArgs);
    emit({ type: "workflow_call_done", name: childName, path, ok: result?.ok !== false });
    return result;
  } finally {
    inNestedWorkflow = false;
    currentPhase = savedPhase;
  }
};

globalThis.pandacode = {
  exec(runtime, prompt, options = {}) {
    return globalThis.agent(prompt, {
      ...options,
      runtime,
      backendRuntime: runtime
    });
  },
  claude(prompt, options = {}) {
    return this.exec("claude", prompt, options);
  },
  codex(prompt, options = {}) {
    return this.exec("codex", prompt, options);
  },
  bamboo(prompt, options = {}) {
    return this.exec("bamboo", prompt, options);
  }
};

globalThis.applyWorktreeDiff = (candidate, options = {}) => applyCapturedWorktreeDiff(candidate, options);
globalThis.applyWorktreeDiffs = (candidates, options = {}) => {
  if (!options.continueOnError) {
    return applyCapturedWorktreeDiffsAtomic(candidates, options);
  }
  const list = Array.isArray(candidates) ? candidates : [candidates];
  const results = [];
  for (let index = 0; index < list.length; index += 1) {
    const result = applyCapturedWorktreeDiff(list[index], {
      ...options,
      label: patchApplyLabel(options, index)
    });
    results.push(result);
  }
  return worktreePatchBatchResult(results);
};
globalThis.reviewWorktreeDiffs = (candidates, options = {}) => reviewCapturedWorktreeDiffs(candidates, options);
globalThis.captureMainWorktreeSnapshot = (options = {}) => captureMainWorktreeSnapshot(options);
globalThis.assertMainWorktreeUnchanged = (snapshot, options = {}) => assertMainWorktreeUnchanged(snapshot, options);
globalThis.restoreMainWorktreeSnapshot = (snapshot, check, options = {}) => restoreMainWorktreeSnapshot(snapshot, check, options);

const mockReviewRejectOnce = new Map();

const WORKTREE_REVIEW_SCHEMA = {
  title: "odw-worktree-review.schema.json",
  type: "object",
  required: ["decision", "summary", "blockers", "risks", "owner_questions", "verification"],
  properties: {
    decision: { enum: ["approve", "reject", "needs_owner"] },
    summary: { type: "string" },
    blockers: { type: "array", items: { type: "string" } },
    risks: { type: "array", items: { type: "string" } },
    owner_questions: { type: "array", items: { type: "string" } },
    verification: { type: "array", items: { type: "string" } },
    files_reviewed: { type: "array", items: { type: "string" } }
  }
};

function createWorktree(baseCwd, options) {
  let gitOk = false;
  try {
    execFileSync("git", ["-C", baseCwd, "rev-parse", "--git-dir"], { stdio: "ignore" });
    gitOk = true;
  } catch {
    gitOk = false;
  }
  if (!gitOk) {
    throw new Error(`isolation:'worktree' requires ${baseCwd} to be a git repository`);
  }
  // Clear git's registry of worktree dirs whose folders no longer exist — orphans
  // a prior crash may have left — before adding a new one. Best-effort, idempotent.
  if (!prunedWorktrees) {
    prunedWorktrees = true;
    try {
      execFileSync("git", ["-C", baseCwd, "worktree", "prune"], { stdio: "ignore" });
    } catch {
      /* prune is best-effort */
    }
  }
  worktreeSeq += 1;
  const label = sanitizeSessionName(options.id || options.nodeId || options.label || "agent");
  const parent = `${runDir}/worktrees`;
  mkdirSync(parent, { recursive: true });
  const dir = `${parent}/${label}-${worktreeSeq}`;
  rmSync(dir, { recursive: true, force: true });
  execFileSync("git", ["-C", baseCwd, "worktree", "add", "--detach", "--quiet", dir], { stdio: "ignore" });
  configureWorktreeExcludes(dir);
  const cleanup = () => {
    try {
      execFileSync("git", ["-C", baseCwd, "worktree", "remove", "--force", dir], { stdio: "ignore" });
    } catch {
      // fall through to manual directory removal below
    }
    rmSync(dir, { recursive: true, force: true });
  };
  return { dir, cleanup };
}

function configureWorktreeExcludes(dir) {
  try {
    const excludePath = execFileSync("git", ["-C", dir, "rev-parse", "--git-path", "info/exclude"], { encoding: "utf8" }).trim();
    if (!excludePath) {
      return;
    }
    mkdirSync(dirname(excludePath), { recursive: true });
    const existing = existsSync(excludePath) ? readFileSync(excludePath, "utf8") : "";
    const lines = [".pandacode/", ".odw/", "node_modules/"];
    const additions = lines.filter((line) => !existing.split(/\r?\n/).includes(line));
    if (additions.length > 0) {
      const prefix = existing && !existing.endsWith("\n") ? "\n" : "";
      writeFileSync(excludePath, `${existing}${prefix}${additions.join("\n")}\n`);
    }
  } catch {
    // Best-effort only: diff capture still excludes executor scratch paths.
  }
}

// After a worktree node runs, capture the agent's file changes as a portable
// patch. Built-in keeps a changed worktree on disk; ODW instead returns the diff
// as data and removes the dir — no orphan worktrees, changes never silently lost.
// Exclude executor/runner scratch from the captured diff so it reflects the
// agent's intended changes, not backend metadata (pandacode logs, odw runs, deps).
const WORKTREE_DIFF_EXCLUDES = [".", ":(exclude).pandacode", ":(exclude).odw", ":(exclude)node_modules"];
function captureWorktreeChanges(dir) {
  try {
    const base = execFileSync("git", ["-C", dir, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
    // Plain `git add -A` silently respects .gitignore (no error on ignored files);
    // the executor-scratch exclusion is applied to status/diff so it works even in
    // a repo that does NOT gitignore .pandacode/.odw.
    execFileSync("git", ["-C", dir, "add", "-A"], { stdio: "ignore" });
    const status = execFileSync("git", ["-C", dir, "status", "--porcelain", "--", ...WORKTREE_DIFF_EXCLUDES], { encoding: "utf8" });
    if (!status.trim()) {
      return { changed: false, files: [], diff: "", base };
    }
    const files = status.trim().split(/\r?\n/).map((line) => line.slice(3).trim()).filter(Boolean);
    const diff = execFileSync("git", ["-C", dir, "diff", "--cached", "HEAD", "--", ...WORKTREE_DIFF_EXCLUDES], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
    return { changed: true, files, diff, base };
  } catch (error) {
    return { changed: false, files: [], diff: "", error: String(error?.message ?? error) };
  }
}

function worktreePatchOf(candidate) {
  if (!candidate || typeof candidate !== "object") {
    return null;
  }
  if (candidate.worktree && typeof candidate.worktree === "object") {
    return candidate.worktree;
  }
  if ("diff" in candidate || "changed" in candidate || "files" in candidate) {
    return candidate;
  }
  return null;
}

function applyCapturedWorktreeDiff(candidate, options = {}) {
  const worktree = worktreePatchOf(candidate);
  const label = options.label || "worktree diff";
  if (!worktree) {
    const result = {
      ok: false,
      applied: false,
      files: [],
      error: { category: "invalid_worktree_diff", message: "Expected an agent result with .worktree or a worktree diff object." }
    };
    emitWorktreePatchApply(label, result);
    return result;
  }

  const files = Array.isArray(worktree.files) ? worktree.files : [];
  if (!worktree.changed || !String(worktree.diff || "").trim()) {
    const result = { ok: true, applied: false, changed: false, files, base: worktree.base || null };
    emitWorktreePatchApply(label, result);
    return result;
  }

  const diff = String(worktree.diff);
  const check = runGitApply(["apply", "--check", "--whitespace=nowarn"], diff);
  if (!check.ok) {
    const result = {
      ok: false,
      applied: false,
      files,
      base: worktree.base || null,
      error: { category: "patch_conflict", message: check.message }
    };
    emitWorktreePatchApply(label, result);
    return result;
  }

  const applied = runGitApply(["apply", "--whitespace=nowarn"], diff);
  if (!applied.ok) {
    const result = {
      ok: false,
      applied: false,
      files,
      base: worktree.base || null,
      error: { category: "patch_apply_failed", message: applied.message }
    };
    emitWorktreePatchApply(label, result);
    return result;
  }

  const result = { ok: true, applied: true, files, base: worktree.base || null };
  emitWorktreePatchApply(label, result);
  return result;
}

function applyCapturedWorktreeDiffsAtomic(candidates, options = {}) {
  const list = Array.isArray(candidates) ? candidates : [candidates];
  const prepared = list.map((candidate, index) => prepareWorktreePatch(candidate, patchApplyLabel(options, index)));
  const invalid = prepared.filter((entry) => entry.result?.ok === false);
  if (invalid.length > 0) {
    const results = prepared.map((entry) => entry.result || {
      ok: false,
      applied: false,
      files: entry.files,
      base: entry.base,
      error: {
        category: "batch_preflight_failed",
        message: "Batch contains an invalid worktree diff; no patch was applied."
      }
    });
    emitWorktreePatchApplyResults(prepared, results);
    return worktreePatchBatchResult(results);
  }

  const changed = prepared.filter((entry) => entry.diff);
  if (changed.length === 0) {
    const results = prepared.map((entry) => entry.result);
    emitWorktreePatchApplyResults(prepared, results);
    return worktreePatchBatchResult(results);
  }

  const combinedDiff = `${changed.map((entry) => entry.diff.trimEnd()).join("\n\n")}\n`;
  const check = runGitApply(["apply", "--check", "--whitespace=nowarn"], combinedDiff);
  if (!check.ok) {
    const results = prepared.map((entry) => entry.result || {
      ok: false,
      applied: false,
      files: entry.files,
      base: entry.base,
      error: { category: "patch_conflict", message: check.message }
    });
    emitWorktreePatchApplyResults(prepared, results);
    return worktreePatchBatchResult(results);
  }

  const applied = runGitApply(["apply", "--whitespace=nowarn"], combinedDiff);
  if (!applied.ok) {
    const results = prepared.map((entry) => entry.result || {
      ok: false,
      applied: false,
      files: entry.files,
      base: entry.base,
      error: { category: "patch_apply_failed", message: applied.message }
    });
    emitWorktreePatchApplyResults(prepared, results);
    return worktreePatchBatchResult(results);
  }

  const results = prepared.map((entry) => entry.result || {
    ok: true,
    applied: true,
    files: entry.files,
    base: entry.base
  });
  emitWorktreePatchApplyResults(prepared, results);
  return worktreePatchBatchResult(results);
}

function prepareWorktreePatch(candidate, label) {
  const worktree = worktreePatchOf(candidate);
  if (!worktree) {
    return {
      label,
      files: [],
      base: null,
      result: {
        ok: false,
        applied: false,
        files: [],
        error: { category: "invalid_worktree_diff", message: "Expected an agent result with .worktree or a worktree diff object." }
      }
    };
  }
  const files = Array.isArray(worktree.files) ? worktree.files : [];
  const base = worktree.base || null;
  const diff = String(worktree.diff || "");
  if (!worktree.changed || !diff.trim()) {
    return {
      label,
      files,
      base,
      result: { ok: true, applied: false, changed: false, files, base }
    };
  }
  return { label, files, base, diff };
}

function patchApplyLabel(options, index) {
  return options.label ? `${options.label}-${index + 1}` : `patch-${index + 1}`;
}

function emitWorktreePatchApplyResults(entries, results) {
  for (let index = 0; index < results.length; index += 1) {
    emitWorktreePatchApply(entries[index]?.label || `patch-${index + 1}`, results[index]);
  }
}

function emitWorktreePatchApply(label, result) {
  const event = {
    type: "worktree_patch_apply",
    label,
    ok: result.ok === true,
    applied: result.applied === true,
    files: Array.isArray(result.files) ? result.files.length : 0
  };
  if (result.error) {
    event.category = result.error.category;
    event.message = truncateText(result.error.message, 240);
  }
  emit(event);
}

function worktreePatchBatchResult(results) {
  const failed = results.filter((result) => result.ok === false);
  const applied = results.filter((result) => result.applied === true);
  return {
    ok: failed.length === 0,
    applied: applied.length,
    failed: failed.length,
    partial: failed.length > 0 && applied.length > 0,
    results
  };
}

async function reviewCapturedWorktreeDiffs(candidates, options = {}) {
  const list = Array.isArray(candidates) ? candidates : [candidates];
  const label = options.label || "worktree-review";
  const prepared = list.map((candidate, index) => prepareWorktreePatch(candidate, patchApplyLabel(options, index)));
  const invalid = prepared.filter((entry) => entry.result?.ok === false);
  const files = uniqueStrings(prepared.flatMap((entry) => entry.files));
  if (invalid.length > 0) {
    const gate = {
      ok: false,
      decision: "reject",
      applyReady: false,
      files,
      preflight: {
        ok: false,
        category: "invalid_worktree_diff",
        message: "One or more candidates are not captured worktree diffs."
      },
      reviews: []
    };
    emitWorktreeReviewGate(label, gate);
    return gate;
  }

  const changed = prepared.filter((entry) => entry.diff);
  if (changed.length === 0) {
    const gate = {
      ok: true,
      decision: "approve",
      applyReady: false,
      files,
      preflight: { ok: true, changed: false },
      reviews: []
    };
    emitWorktreeReviewGate(label, gate);
    return gate;
  }

  const combinedDiff = combinedWorktreeDiff(changed);
  const check = runGitApply(["apply", "--check", "--whitespace=nowarn"], combinedDiff);
  if (!check.ok) {
    const gate = {
      ok: false,
      decision: "reject",
      applyReady: false,
      files,
      preflight: {
        ok: false,
        category: "patch_conflict",
        message: check.message
      },
      reviews: []
    };
    emitWorktreeReviewGate(label, gate);
    return gate;
  }

  let reviewWorktree = null;
  let reviewWorkspaceReady = false;
  try {
    try {
      reviewWorktree = createWorktree(cwd, { id: `${label}-candidate`, label: `${label}-candidate` });
    } catch (error) {
      const gate = {
        ok: false,
        decision: "reject",
        applyReady: false,
        files,
        preflight: {
          ok: false,
          category: "review_workspace_failed",
          message: String(error?.message ?? error)
        },
        reviews: []
      };
      emitWorktreeReviewGate(label, gate);
      return gate;
    }
    const reviewApply = runGitApplyIn(reviewWorktree.dir, ["apply", "--whitespace=nowarn"], combinedDiff);
    if (!reviewApply.ok) {
      const gate = {
        ok: false,
        decision: "reject",
        applyReady: false,
        files,
        preflight: {
          ok: false,
          category: "review_workspace_failed",
          message: reviewApply.message
        },
        reviews: []
      };
      emitWorktreeReviewGate(label, gate);
      return gate;
    }
    reviewWorkspaceReady = true;
    emit({ type: "worktree_review_workspace", label, status: "start", dir: reviewWorktree.dir, files: files.length });

    const reviewers = normalizeWorktreeReviewers(options);
    const basePrompt = buildWorktreeReviewPrompt({ prepared: changed, combinedDiff, files, options });
    const reviews = await globalThis.parallel(
      reviewers.map((reviewer, index) => () =>
        globalThis.agent(buildReviewerPrompt(basePrompt, reviewer), {
          id: reviewer.id || `${label}-review-${index + 1}`,
          label: reviewer.label || `review-${index + 1}`,
          phase: options.phase,
          runtime: reviewer.runtime || options.runtime || "codex",
          provider: reviewer.provider || options.provider,
          permission: reviewer.permission || options.permission || "limited",
          model: reviewer.model || options.model,
          effort: reviewer.effort || options.effort,
          timeout: reviewer.timeout || options.timeout,
          execCwd: reviewWorktree.dir,
          schema: WORKTREE_REVIEW_SCHEMA,
          schemaDescription: "Final response is the structured ODW worktree diff review gate verdict.",
          retry: reviewer.retry || options.retry || { maxAttempts: 2 }
        })
      ),
      { label: `${label}-review`, max: options.maxReviewers || reviewers.length }
    );
    const normalizedReviews = reviews.map((review, index) => normalizeWorktreeReview(review, reviewers[index]));
    const gate = aggregateWorktreeReviewGate({ label, files, reviews: normalizedReviews });
    emitWorktreeReviewGate(label, gate);
    return gate;
  } finally {
    if (reviewWorktree) {
      reviewWorktree.cleanup();
      if (reviewWorkspaceReady) {
        emit({ type: "worktree_review_workspace", label, status: "done", files: files.length });
      }
    }
  }
}

function combinedWorktreeDiff(entries) {
  return `${entries.map((entry) => String(entry.diff || "").trimEnd()).filter(Boolean).join("\n\n")}\n`;
}

function normalizeWorktreeReviewers(options = {}) {
  if (Array.isArray(options.reviewers) && options.reviewers.length > 0) {
    return options.reviewers.map((reviewer, index) =>
      typeof reviewer === "string"
        ? { label: reviewer, perspective: reviewer }
        : { label: `review-${index + 1}`, ...(reviewer || {}) }
    );
  }
  const count = Math.max(1, Math.min(4, Number(options.reviewerCount || 1)));
  const defaultPerspectives = [
    "correctness and regression risk",
    "adversarial edge-case review",
    "product intent and owner decision risk",
    "verification evidence review"
  ];
  return Array.from({ length: count }, (_, index) => ({
    label: count === 1 ? "review" : `review-${index + 1}`,
    perspective: defaultPerspectives[index] || "general review",
    runtime: options.runtime || "codex",
    permission: options.permission || "limited"
  }));
}

function buildWorktreeReviewPrompt({ prepared, combinedDiff, files, options }) {
  const diffLimit = Math.max(2000, Number(options.maxDiffChars || 30000));
  const diffText = truncateText(combinedDiff, diffLimit, "head");
  const context = options.context ? `\nProject context:\n${String(options.context)}\n` : "";
  const criteria = Array.isArray(options.criteria) && options.criteria.length > 0
    ? options.criteria.map((item) => `- ${item}`).join("\n")
    : "- Check whether this batch is safe to land atomically.\n- Identify blockers, missing verification, semantic conflicts, and owner decisions.\n- Prefer needs_owner when product intent or acceptance criteria require human judgment.";
  return `Review an ODW batch of captured worktree diffs before atomic landing.

The combined diff has already been applied to your current working directory for
this review node. Inspect the files and run relevant tests/checks there. Do not
edit files.

New files from the captured diff may appear as untracked in git status inside
this temporary review workspace. Do not treat that alone as a landing blocker:
approval lands the captured patch with applyWorktreeDiffs, including new files
listed below.

Files:
${files.map((file) => `- ${file}`).join("\n")}

Base commits:
${uniqueStrings(prepared.map((entry) => entry.base).filter(Boolean)).map((base) => `- ${base}`).join("\n") || "- unknown"}
${context}
Review criteria:
${criteria}

Return decision:
- approve: safe to apply atomically after this gate.
- reject: do not apply; blockers or failed verification must be fixed first.
- needs_owner: owner/product decision is required before AI should land the batch.

Combined diff:
${diffText}`;
}

function buildReviewerPrompt(basePrompt, reviewer) {
  const perspective = reviewer.perspective ? `\nReviewer perspective: ${reviewer.perspective}\n` : "";
  return `${basePrompt}${perspective}
Be adversarial and evidence-backed. Do not edit files.`;
}

function normalizeWorktreeReview(review, reviewer = {}) {
  if (!review || review.ok === false) {
    return {
      reviewer: reviewer.label || "review",
      decision: "reject",
      summary: firstText(review?.error?.message, "reviewer failed or returned no result"),
      blockers: [firstText(review?.error?.message, "reviewer failed or returned no result")],
      risks: [],
      owner_questions: [],
      verification: [],
      files_reviewed: []
    };
  }
  if (typeof review === "object" && !Array.isArray(review)) {
    const decision = ["approve", "reject", "needs_owner"].includes(review.decision) ? review.decision : inferReviewDecision(review.summary || "");
    return {
      reviewer: reviewer.label || "review",
      decision,
      summary: firstText(review.summary, JSON.stringify(review).slice(0, 1000)),
      blockers: stringArray(review.blockers),
      risks: stringArray(review.risks),
      owner_questions: stringArray(review.owner_questions),
      verification: stringArray(review.verification),
      files_reviewed: stringArray(review.files_reviewed)
    };
  }
  const text = String(review);
  return {
    reviewer: reviewer.label || "review",
    decision: inferReviewDecision(text),
    summary: text.slice(0, 1000),
    blockers: /reject|fail|blocker|失败|拒绝|不通过/i.test(text) ? [text.slice(0, 1000)] : [],
    risks: [],
    owner_questions: /needs_owner|owner|拍板|决策/i.test(text) ? [text.slice(0, 1000)] : [],
    verification: [],
    files_reviewed: []
  };
}

function inferReviewDecision(text) {
  const value = String(text || "");
  if (/needs_owner|owner|拍板|决策|需要.*确认/i.test(value)) {
    return "needs_owner";
  }
  if (/reject|fail|blocker|failed|失败|拒绝|不通过|阻塞/i.test(value)) {
    return "reject";
  }
  return "approve";
}

function aggregateWorktreeReviewGate({ files, reviews }) {
  const rejected = reviews.filter((review) => review.decision === "reject");
  const owner = reviews.filter((review) => review.decision === "needs_owner");
  const decision = rejected.length > 0 ? "reject" : (owner.length > 0 ? "needs_owner" : "approve");
  const ok = decision === "approve";
  return {
    ok,
    decision,
    applyReady: ok,
    files,
    preflight: { ok: true, changed: true },
    reviews,
    blockers: uniqueStrings(reviews.flatMap((review) => review.blockers)),
    risks: uniqueStrings(reviews.flatMap((review) => review.risks)),
    owner_questions: uniqueStrings(reviews.flatMap((review) => review.owner_questions)),
    verification: uniqueStrings(reviews.flatMap((review) => review.verification))
  };
}

function emitWorktreeReviewGate(label, gate) {
  emit({
    type: "worktree_review_gate",
    label,
    ok: gate.ok === true,
    decision: gate.decision,
    applyReady: gate.applyReady === true,
    files: Array.isArray(gate.files) ? gate.files.length : 0,
    file_samples: previewStrings(gate.files, 8, 160),
    reviewers: Array.isArray(gate.reviews) ? gate.reviews.length : 0,
    review_decisions: previewStrings(
      (gate.reviews || []).map((review) => `${review.reviewer || "review"}:${review.decision || "unknown"}`),
      8,
      160
    ),
    blockers: Array.isArray(gate.blockers) ? gate.blockers.length : 0,
    blocker_samples: previewStrings(gate.blockers, 5, 500),
    risks: Array.isArray(gate.risks) ? gate.risks.length : 0,
    risk_samples: previewStrings(gate.risks, 5, 500),
    owner_questions: Array.isArray(gate.owner_questions) ? gate.owner_questions.length : 0,
    owner_question_samples: previewStrings(gate.owner_questions, 5, 500),
    verification_samples: previewStrings(gate.verification, 5, 500)
  });
}

function stringArray(value) {
  return Array.isArray(value) ? value.filter((item) => typeof item === "string").map((item) => item.trim()).filter(Boolean) : [];
}

function previewStrings(values, maxItems = 5, maxChars = 300) {
  return stringArray(values).slice(0, maxItems).map((value) =>
    value.length > maxChars ? `${value.slice(0, maxChars - 1)}…` : value
  );
}

function uniqueStrings(values) {
  return [...new Set(stringArray(values))];
}

function captureMainWorktreeSnapshot(options = {}) {
  return {
    label: options.label || "",
    ...captureGitSnapshot(cwd)
  };
}

function assertMainWorktreeUnchanged(snapshot, options = {}) {
  const label = options.label || snapshot?.label || "main-worktree";
  const before = snapshot && typeof snapshot === "object" ? snapshot : { ok: false, files: [], hashes: {} };
  const after = captureGitSnapshot(cwd);
  const beforeHashes = before.hashes && typeof before.hashes === "object" ? before.hashes : {};
  const afterHashes = after.hashes && typeof after.hashes === "object" ? after.hashes : {};
  const beforeFiles = new Set(Object.keys(beforeHashes));
  const afterFiles = new Set(Object.keys(afterHashes));
  const added = [...afterFiles].filter((file) => !beforeFiles.has(file)).sort();
  const removed = [...beforeFiles].filter((file) => !afterFiles.has(file)).sort();
  const modified = [...afterFiles].filter((file) => beforeFiles.has(file) && beforeHashes[file] !== afterHashes[file]).sort();
  const files = uniqueStrings([...added, ...removed, ...modified]);
  const result = {
    ok: before.ok === true && after.ok === true && files.length === 0,
    label,
    before_files: before.files?.length || 0,
    after_files: after.files?.length || 0,
    added,
    removed,
    modified,
    files,
    error: before.error || after.error || undefined
  };
  emit({
    type: "worktree_snapshot_check",
    label,
    ok: result.ok,
    files: files.length,
    file_samples: previewStrings(files, 8, 160),
    added: added.length,
    removed: removed.length,
    modified: modified.length,
    message: result.error
  });
  return result;
}

function restoreMainWorktreeSnapshot(snapshot, check = null, options = {}) {
  const label = options.label || snapshot?.label || "main-worktree-restore";
  const before = snapshot && typeof snapshot === "object" ? snapshot : { ok: false, files: [], hashes: {}, contents: {} };
  const detected = check && typeof check === "object" ? check : assertMainWorktreeUnchanged(snapshot, { label: `${label}-precheck` });
  const contents = before.contents && typeof before.contents === "object" ? before.contents : {};
  const restored = [];
  const removed = [];
  const errors = [];

  for (const file of stringArray(detected.added)) {
    try {
      rmSync(`${cwd}/${file}`, { force: true });
      removed.push(file);
    } catch (error) {
      errors.push(`${file}: ${String(error?.message ?? error)}`);
    }
  }

  for (const file of uniqueStrings([...(detected.modified || []), ...(detected.removed || [])])) {
    try {
      const encoded = contents[file];
      const path = `${cwd}/${file}`;
      if (encoded === null || encoded === undefined) {
        rmSync(path, { force: true });
        removed.push(file);
      } else {
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, Buffer.from(String(encoded), "base64"));
        restored.push(file);
      }
    } catch (error) {
      errors.push(`${file}: ${String(error?.message ?? error)}`);
    }
  }

  const after = assertMainWorktreeUnchanged(snapshot, { label: `${label}-after` });
  const result = {
    ok: errors.length === 0 && after.ok === true,
    label,
    restored,
    removed,
    errors,
    after
  };
  emit({
    type: "worktree_snapshot_restore",
    label,
    ok: result.ok,
    restored: restored.length,
    removed: removed.length,
    files: uniqueStrings([...restored, ...removed]).length,
    file_samples: previewStrings([...restored, ...removed], 8, 160),
    message: errors.join("; ") || undefined
  });
  return result;
}

function captureGitSnapshot(dir) {
  try {
    const files = gitChangedFiles(dir);
    const hashes = {};
    const contents = {};
    for (const file of files) {
      const path = `${dir}/${file}`;
      if (existsSync(path)) {
        const content = readFileSync(path);
        hashes[file] = createHash("sha256").update(content).digest("hex");
        contents[file] = content.toString("base64");
      } else {
        hashes[file] = null;
        contents[file] = null;
      }
    }
    return { ok: true, files, hashes, contents };
  } catch (error) {
    return { ok: false, files: [], hashes: {}, contents: {}, error: String(error?.message ?? error) };
  }
}

function gitChangedFiles(dir) {
  const tracked = execFileSync("git", ["-C", dir, "diff", "--name-only", "HEAD", "--", ...WORKTREE_DIFF_EXCLUDES], {
    encoding: "utf8",
    maxBuffer: 8 * 1024 * 1024
  });
  const untracked = execFileSync("git", ["-C", dir, "ls-files", "--others", "--exclude-standard", "--", ...WORKTREE_DIFF_EXCLUDES], {
    encoding: "utf8",
    maxBuffer: 8 * 1024 * 1024
  });
  return uniqueStrings(`${tracked}\n${untracked}`.split(/\r?\n/));
}

function runGitApply(args, input) {
  return runGitApplyIn(cwd, args, input);
}

function runGitApplyIn(dir, args, input) {
  try {
    execFileSync("git", ["-C", dir, ...args], { input, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
    return { ok: true, message: "" };
  } catch (error) {
    return {
      ok: false,
      message: firstText(error?.stderr?.toString?.(), error?.stdout?.toString?.(), error?.message, "git apply failed")
    };
  }
}

async function runAgent(prompt, options) {
  if (options.isolation === "worktree") {
    const wt = createWorktree(cwd, options);
    emit({ type: "worktree_start", label: options.label, phase: options.phase, dir: wt.dir });
    let changes = { changed: false, files: [], diff: "" };
    try {
      const result = await dispatchBackend(prompt, { ...options, execCwd: wt.dir });
      changes = captureWorktreeChanges(wt.dir);
      if (result && typeof result === "object" && !Array.isArray(result)) {
        result.__worktree = changes;
      }
      return result;
    } finally {
      wt.cleanup();
      emit({ type: "worktree_done", label: options.label, phase: options.phase, dir: wt.dir, changed: changes.changed, files: changes.files.length });
    }
  }
  return dispatchBackend(prompt, options);
}

async function dispatchBackend(prompt, options) {
  if (backend === "mock") {
    // Mock-only: `mockAgentText` makes the node return a real-executor (pandacode)
    // report envelope, so the no-schema lean-collapse path (report -> final text /
    // {text, worktree}) is exercised without a live executor.
    const mockReport = (!schemaNameOf(options.schema) && options.mockAgentText !== undefined)
      ? {
        ok: true,
        backend: "pandacode",
        runtime: options.runtime || "codex",
        state: "completed",
        run_id: "mock-run",
        thread_id: "mock-thread",
        last_agent_message: String(options.mockAgentText)
      }
      : null;
    const mockResult = mockReport || mockResultForSchema(options, prompt) || {
      ok: true,
      action: options.action || "mock",
      backend: "mock",
      agentType: options.agentType,
      label: options.label,
      phase: options.phase,
      prompt_preview: String(prompt).slice(0, 240)
    };
    // Opt-in synthetic token usage so budget loops/ceilings are testable for free,
    // on the SAME field path the real codex backend reports (codex.usage...totalTokens).
    const mockTokens = Number(options.mockTokens);
    if (Number.isFinite(mockTokens) && mockTokens > 0 && mockResult && typeof mockResult === "object" && !mockResult.codex) {
      mockResult.codex = { usage: { tokenUsage: { total: { totalTokens: mockTokens } } } };
    }
    // Mock-only: force a node failure (optionally retryable) so retry, failure
    // propagation, and budget-accrual-on-failed-attempts are testable for free.
    if (options.mockFail && mockResult && typeof mockResult === "object") {
      mockResult.ok = false;
      mockResult.state = "failed";
      mockResult.error = {
        category: "mock_failed",
        message: "mock forced failure",
        retryable: Boolean(options.mockRetryable)
      };
    }
    // Mock-only: stand in for an executor that resolved a concrete model the
    // script left implicit, so the model-backfill (inherit -> real) is testable.
    if (options.mockResolvedModel && mockResult && typeof mockResult === "object") {
      mockResult.summary = { ...(mockResult.summary || {}), model: String(options.mockResolvedModel) };
    }
    // Mock-only: write the requested file plus an executor-scratch file under
    // .pandacode/ so diff-capture and read-only guards are testable for free.
    if (options.mockWriteFile) {
      const mockCwd = options.execCwd || cwd;
      const mockWritePath = `${mockCwd}/${options.mockWriteFile}`;
      mkdirSync(dirname(mockWritePath), { recursive: true });
      writeFileSync(mockWritePath, `mock change by ${options.label || "agent"}\n`);
      mkdirSync(`${mockCwd}/.pandacode`, { recursive: true });
      writeFileSync(`${mockCwd}/.pandacode/scratch.txt`, "executor metadata that must not pollute the captured diff\n");
    }
    return mockResult;
  }
  if (backend === "pandacode") {
    return runPandaCode(prompt, options);
  }
  throw new Error(`unsupported ODW exec backend: ${backend}`);
}

function mockResultForSchema(options, prompt) {
  const schemaName = schemaNameOf(options.schema);
  if (schemaName.endsWith("research.schema.json")) {
    return {
      summary: "mock research result",
      files: [],
      batches: [{ name: "mock", files: [], notes: String(prompt).slice(0, 120) }],
      evidence: []
    };
  }
  if (schemaName.endsWith("security-finding.schema.json")) {
    return { findings: [], clean_files: [], uncertain: [] };
  }
  if (schemaName.endsWith("odw-worktree-review.schema.json")) {
    const text = String(prompt || "");
    const repeatReject = text.match(/MOCK_REJECT_(ONCE|TWICE)/)?.[1];
    if (repeatReject) {
      const key = options.label || options.id || "review";
      const seen = mockReviewRejectOnce.get(key) || 0;
      mockReviewRejectOnce.set(key, seen + 1);
      const rejectLimit = repeatReject === "TWICE" ? 2 : 1;
      if (seen < rejectLimit) {
        const file = text.match(/MOCK_REJECT_(?:ONCE|TWICE)_FILE:([^\s]+)/)?.[1];
        const blocker = text.match(/MOCK_REJECT_(?:ONCE|TWICE)_BLOCKER:([^\n]+)/)?.[1]?.trim();
        return {
          decision: "reject",
          summary: `mock review rejected attempt ${seen + 1}`,
          blockers: [blocker || (file ? `mock one-time blocker in ${file}` : "mock one-time blocker")],
          risks: [],
          owner_questions: [],
          verification: ["mock preflight passed"],
          files_reviewed: []
        };
      }
    }
    if (/MOCK_NEEDS_OWNER/.test(text)) {
      return {
        decision: "needs_owner",
        summary: "mock review needs owner decision",
        blockers: [],
        risks: ["mock owner-sensitive product decision"],
        owner_questions: ["mock owner question"],
        verification: ["mock preflight passed"],
        files_reviewed: []
      };
    }
    if (/\bMOCK_REJECT\b/.test(text)) {
      return {
        decision: "reject",
        summary: "mock review rejected the batch",
        blockers: ["mock blocker"],
        risks: [],
        owner_questions: [],
        verification: ["mock preflight passed"],
        files_reviewed: []
      };
    }
    return {
      decision: "approve",
      summary: "mock review approved the batch",
      blockers: [],
      risks: [],
      owner_questions: [],
      verification: ["mock preflight passed"],
      files_reviewed: []
    };
  }
  if (schemaName.endsWith("codex-plan.schema.json")) {
    return {
      status: "planned",
      plan: { summary: "mock plan", steps: ["mock step"], files: [] },
      constraints: [],
      verification: []
    };
  }
  if (schemaName.endsWith("task-plan.schema.json")) {
    return {
      status: "planned",
      summary: "mock decomposed workflow plan",
      tasks: [
        {
          id: "task-a",
          title: "Mock task A",
          prompt: "Implement mock task A.",
          agentType: "odw-codex-coder",
          depends_on: [],
          files: ["mock-a.txt"],
          verification: ["mock verify A"]
        },
        {
          id: "task-b",
          title: "Mock task B",
          prompt: "Implement mock task B.",
          agentType: "odw-codex-coder",
          depends_on: [],
          files: ["mock-b.txt"],
          verification: ["mock verify B"]
        },
        {
          id: "task-c",
          title: "Mock task C",
          prompt: "Implement mock task C.",
          agentType: "odw-codex-coder",
          depends_on: [],
          files: ["mock-c.txt"],
          verification: ["mock verify C"]
        }
      ],
      join: { strategy: "all", expected_outputs: ["codex evidence", "verification evidence"] },
      quality: { max_rework_iterations: 1, acceptance: ["all tasks completed", "review accepts evidence"] }
    };
  }
  if (schemaName.endsWith("codex-result.schema.json")) {
    return {
      run_id: "mock-run",
      status: "completed",
      changed_files: [],
      verification: [],
      risks: [],
      adapter: { backend: "pandacode", runtime: options.runtime || inferPandaRuntime(options) },
      error: null
    };
  }
  if (schemaName.endsWith("test-result.schema.json")) {
    return { commands: [], verdict: "passed" };
  }
  if (schemaName.endsWith("verifier.schema.json")) {
    return { accepted: [], rejected: [], needs_more_evidence: [] };
  }
  if (schemaName.endsWith("task-join.schema.json")) {
    return {
      status: "joined",
      summary: "mock joined implementation results",
      items: [],
      failed: [],
      review_targets: [
        { id: "task-a", title: "Mock task A review", evidence: "mock evidence A" },
        { id: "task-b", title: "Mock task B review", evidence: "mock evidence B" },
        { id: "task-c", title: "Mock task C review", evidence: "mock evidence C" }
      ]
    };
  }
  if (schemaName.endsWith("quality-gate.schema.json")) {
    return {
      verdict: "pass",
      score: 1,
      accepted: ["mock quality gate passed"],
      issues: [],
      rework_tasks: [],
      next_action: "synthesize"
    };
  }
  if (schemaName.endsWith("synthesis.schema.json")) {
    return { summary: "mock synthesis", details: [], risks: [], next_actions: [] };
  }
  if (schemaName.endsWith("error-feedback.schema.json")) {
    return {
      ok: false,
      origin: { phase: options.phase || "", agent: options.label || "mock" },
      error: { category: "unknown", message: "mock error feedback" },
      feedback: { retryable: false, user_message: "mock", next_action: "none" }
    };
  }
  return null;
}

function usageTotalTokens(usage) {
  if (!usage || typeof usage !== "object") {
    return null;
  }
  const value = usage?.tokenUsage?.total?.totalTokens
    ?? usage?.token_usage?.total?.total_tokens
    ?? usage?.total?.totalTokens
    ?? usage?.total?.total_tokens
    ?? usage?.totalTokens
    ?? usage?.total_tokens
    ?? null;
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

function codexTotalTokens(report) {
  return usageTotalTokens(report?.codex?.usage) ?? usageTotalTokens(report?.usage);
}

// Total tokens for one node result across backends: codex (report.codex.usage),
// pandacode (report.execute.summary.usage / report.summary.usage), or top-level usage.
function nodeTotalTokens(result) {
  const direct = codexTotalTokens(result);
  if (direct != null) {
    return direct;
  }
  const pandaUsage = result?.execute?.summary?.usage || result?.summary?.usage;
  if (pandaUsage) {
    return usageTotalTokens(pandaUsage);
  }
  return null;
}

// When the script omits options.model, the executor still resolves a concrete
// model (bamboo -> summary.model, claude -> model/summary.model, codex similar).
// Recover it so the journal + HTML report show what actually ran, not "inherit".
function resolvedModelFromReport(result) {
  if (!result || typeof result !== "object") {
    return null;
  }
  const candidates = [
    result.summary?.model,
    result.model,
    result.execute?.summary?.model,
    result.codex?.model,
    result.start?.model
  ];
  for (const candidate of candidates) {
    if (typeof candidate === "string" && candidate.trim() && candidate !== "inherit") {
      return candidate.trim();
    }
  }
  return null;
}

// Accrue a finished node's real token usage into the run budget. Nodes without
// token info (e.g. claude/tmux) contribute 0 and flag spent() as a floor.
function accrueBudget(rawResult) {
  if (!state.budget) {
    return;
  }
  const tokens = nodeTotalTokens(rawResult);
  if (typeof tokens === "number" && Number.isFinite(tokens)) {
    state.budget.spent = (state.budget.spent ?? 0) + tokens;
  } else {
    state.budget.approx = true;
  }
}

// pandacode <runtime> exec pauses at needs_input; auto-answer (pick the first /
// recommended option) and continue, bounded, so single-shot nodes still complete
// tasks where Codex asks a clarifying question.
async function autoAnswerNeedsInput(result, runtime, fallbackSession, execCwd, timeoutMs) {
  const MAX_ROUNDS = 6;
  for (let round = 1; round <= MAX_ROUNDS; round += 1) {
    const category = String(result?.error?.category || "");
    const needsInput = result
      && (result.state === "waiting_for_user" || category.includes("needs_input"));
    if (!needsInput) {
      return result;
    }
    const session = result.session || fallbackSession;
    if (!session) {
      return result;
    }
    emit({ type: "panda_auto_answer", runtime, session, round });
    const answerArgs = [
      runtime,
      "answer",
      "--cd",
      execCwd,
      "--session",
      session,
      "--text",
      "Proceed and complete the task now with reasonable default choices; make all required file edits. Do not ask further questions.",
      "--wait",
      "--json"
    ];
    if (timeoutMs) {
      answerArgs.push("--timeout-ms", String(timeoutMs));
    }
    if (runtime === "codex") {
      answerArgs.push("--codexctl-bin", codexctlBin);
    }
    result = await runPandaCodeCommand(runtime, "answer", answerArgs, execCwd);
  }
  return result;
}

async function runPandaCode(prompt, options) {
  const execCwd = options.execCwd || cwd;
  const runtime = inferPandaRuntime(options);
  const promptFile = writePromptFile(prompt, { ...options, label: `${runtime}-${options.label || options.id || "agent"}` });
  const session = sanitizeSessionName(
    options.session
    || options.sessionName
    || `${runId}-${options.id || options.nodeId || options.label || "agent"}-${options.attempt || 1}`
  );
  const selectedProvider = options.provider || options.bambooProvider || provider;
  const args = [
    runtime,
    "exec"
  ];
  if (selectedProvider) {
    if (runtime !== "bamboo") {
      throw new Error(`provider is only supported for PandaCode Bamboo nodes; got runtime=${runtime}`);
    }
    args.push("--provider", String(selectedProvider));
  }
  args.push(
    "--cd",
    execCwd,
    "--session",
    session,
    "--task-file",
    promptFile,
    "--json"
  );
  const selectedModel = options.model || model;
  if (selectedModel) {
    args.push("--model", selectedModel);
  }
  const selectedEffort = options.effort || effort;
  if (selectedEffort) {
    args.push("--effort", selectedEffort);
  }
  // options.timeoutMs is already in milliseconds; options.timeout / the CLI
  // --timeout default are in SECONDS. Keep the units separate so a small ms value
  // is not silently multiplied by 1000.
  let timeoutMs;
  if (options.timeoutMs != null && options.timeoutMs !== "") {
    timeoutMs = parseTimeout(options.timeoutMs, "ms");
  } else if (options.timeout != null && options.timeout !== "") {
    timeoutMs = parseTimeout(options.timeout, "s");
  } else {
    // No per-node timeout pinned. Real codex coding tasks (implement / fix / run
    // tests) routinely run for minutes, so a 120s default silently truncates
    // them. Floor codex at 10 min; bamboo/claude (chat) keep the run-level
    // default. An explicit options.timeout / --timeout still wins above.
    const base = parseTimeout(timeout, "s");
    timeoutMs = runtime === "codex" ? Math.max(base ?? 0, 600_000) : base;
  }
  if (timeoutMs) {
    args.push("--timeout-ms", String(timeoutMs));
  }
  if (runtime === "codex") {
    args.push("--codexctl-bin", codexctlBin);
    // Default to full access because a coding node usually must install
    // dependencies (npm/pip/cargo) and reach the network, and the only narrower
    // mode codexctl exposes — workspace-write — also BLOCKS network, which breaks
    // real builds (verified: `npm install` fails with connect EPERM under it).
    // Authors can opt a node down with { permission: "limited" } to confine it to
    // the working dir with no network (good for reviewing/analysing code).
    const permission = options.permission === "limited" ? "limited" : "max";
    args.push("--permission", permission);
  }
  const result = await runPandaCodeCommand(runtime, "exec", args, execCwd);
  return autoAnswerNeedsInput(result, runtime, session, execCwd, timeoutMs);
}

function inferPandaRuntime(options) {
  const explicit = options.runtime || options.backendRuntime || options.modelRuntime;
  if (explicit) {
    return String(explicit).toLowerCase();
  }
  if (options.provider || options.bambooProvider || provider) {
    return "bamboo";
  }
  const agentType = String(options.agentType || "").toLowerCase();
  if (agentType.includes("codex") || ["start", "execute", "read", "answer"].includes(options.action || "")) {
    return "codex";
  }
  return "claude";
}

// Parse a timeout to milliseconds. `unit` interprets a bare number: "ms" as-is,
// "s" as seconds. Empty / "none" / "unlimited" / non-positive -> null (no limit).
function parseTimeout(value, unit) {
  if (value === undefined || value === null || value === "" || value === "none" || value === "unlimited") {
    return null;
  }
  const number = Number(value);
  if (!Number.isFinite(number) || number <= 0) {
    return null;
  }
  return unit === "ms" ? Math.round(number) : Math.round(number * 1000);
}

function sanitizeSessionName(value) {
  return String(value || "odw-agent")
    .replace(/[^a-zA-Z0-9_.-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 180) || "odw-agent";
}

function runPandaCodeCommand(runtime, action, args, execCwd = cwd) {
  return new Promise((resolve) => {
    const child = spawn(pandacodeBin, args, {
      cwd: execCwd,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"]
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", (error) => {
      resolve({
        ok: false,
        backend: "pandacode",
        runtime,
        action,
        exit_code: null,
        stdout_tail: stdout.slice(-4000),
        stderr_tail: stderr.slice(-4000),
        error: { category: "pandacode_failed", message: String(error?.message ?? error) }
      });
    });
    child.on("close", (code) => {
      const parsed = parsePandaCodeReportFromStdout(stdout) || parseJsonObjectFromText(stdout);
      if (parsed) {
        resolve(normalizePandaCodeReport(parsed, { runtime, action, exit_code: code, stdout, stderr }));
        return;
      }
      resolve({
        ok: code === 0,
        backend: "pandacode",
        runtime,
        action,
        exit_code: code,
        stdout_tail: stdout.slice(-4000),
        stderr_tail: stderr.slice(-4000),
        error: code === 0 ? null : { category: "pandacode_failed", message: stderr || stdout || "pandacode failed" }
      });
    });
  });
}

function parsePandaCodeReportFromStdout(stdout) {
  for (const line of String(stdout || "").trim().split(/\r?\n/).reverse()) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) {
      continue;
    }
    const parsed = parseJsonObjectFromText(trimmed);
    if (looksLikePandaCodeReport(parsed)) {
      return parsed;
    }
  }
  return null;
}

function looksLikePandaCodeReport(value) {
  return Boolean(
    value
    && typeof value === "object"
    && !Array.isArray(value)
    && (
      "ok" in value
      || "summary" in value
      || "record" in value
      || "session" in value
      || "state" in value
    )
  );
}

function normalizePandaCodeReport(report, context) {
  if (!report || typeof report !== "object") {
    return report;
  }
  const runtime = report.runtime || context.runtime;
  const action = report.action || context.action;
  const rawReportPath = writePandaCodeRawReport(report, { runtime, action });
  const record = compactPandaRecord(report.record);
  const summary = compactPandaSummary(report.summary);
  const start = compactPandaCommand(report.start);
  const execute = compactPandaCommand(report.execute);
  const artifacts = compactPandaArtifacts(record?.artifacts || report.artifacts);
  const domainFields = compactPandaDomainFields(report);
  if (rawReportPath) {
    artifacts.raw_report = rawReportPath;
  }
  const lastAgentMessage = truncateText(
    readPandaCodeLastAssistantMessage(report)
      || summary?.last_agent_message
      || start?.last_agent_message
      || execute?.last_agent_message
      || "",
    4000
  );
  const error = compactPandaError(report.error || start?.error || execute?.error);
  // A non-zero process exit means the executor failed, even when its JSON report
  // omits `ok` or optimistically reports ok:true. odw's core job is to surface
  // pandacode failures, so a non-zero exit overrides an absent/true report.ok.
  const exitFailed = typeof context.exit_code === "number" && context.exit_code !== 0;
  const normalized = {
    ok: report.ok === false || exitFailed ? false : report.ok,
    backend: "pandacode",
    runtime,
    // Preserve the model the executor actually used, read from the raw report
    // before compaction drops it, so observability can show it (vs "inherit").
    model: report.summary?.model || report.record?.model || report.model || undefined,
    action,
    session: report.session || record?.session || "",
    state: report.state || summary?.status || "unknown",
    exit_code: context.exit_code,
    run_id: report.run_id || report.runId || record?.run_id || summary?.run_id || report.session || "",
    thread_id: report.thread_id || report.threadId || record?.thread_id || summary?.thread_id,
    thread_path: report.thread_path || report.threadPath || record?.thread_path || summary?.thread_path,
    last_agent_message: lastAgentMessage,
    summary: compactPandaNodeSummary(summary, { start, execute }),
    ...domainFields,
    artifacts,
    adapter: {
      backend: "pandacode",
      runtime,
      raw_report: rawReportPath || undefined,
      log_path: firstText(summary?.log_path, start?.log_path, execute?.log_path),
      stderr_tail: firstText(execute?.stderr_tail, start?.stderr_tail).slice(-600)
    }
  };
  if (error) {
    normalized.error = error;
  }
  if (normalized.ok === false || exitFailed) {
    normalized.stdout_tail = truncateText(String(context.stdout || ""), 1000, "tail");
    normalized.stderr_tail = truncateText(String(context.stderr || ""), 1000, "tail");
  }
  if (normalized.ok === false && !normalized.error) {
    normalized.error = {
      category: normalized.state === "waiting_for_user" ? "pandacode_needs_input" : "pandacode_failed",
      message: firstText(execute?.stderr_tail, start?.stderr_tail, normalized.stderr_tail, normalized.stdout_tail, exitFailed ? `pandacode exited ${context.exit_code}` : "pandacode node failed").slice(0, 2000),
      retryable: normalized.state !== "failed"
    };
  }
  return pruneEmpty(normalized);
}

function compactPandaDomainFields(report) {
  const metaKeys = new Set([
    "ok",
    "backend",
    "runtime",
    "action",
    "session",
    "state",
    "exit_code",
    "run_id",
    "runId",
    "thread_id",
    "threadId",
    "thread_path",
    "threadPath",
    "record",
    "start",
    "execute",
    "artifacts",
    "adapter",
    "error",
    "stdout",
    "stderr",
    "stdout_tail",
    "stderr_tail",
    "last_agent_message"
  ]);
  const output = {};
  for (const [key, value] of Object.entries(report)) {
    if (metaKeys.has(key) || value === undefined || typeof value === "function") {
      continue;
    }
    if (
      key === "summary"
      && value
      && typeof value === "object"
      && !Array.isArray(value)
      && ("last_agent_message" in value || "log_path" in value || "status" in value)
    ) {
      continue;
    }
    output[key] = value;
  }
  return output;
}

function writePandaCodeRawReport(report, { runtime, action }) {
  try {
    const session = sanitizeSessionName(report.session || report.record?.session || `${runtime || "runtime"}-${action || "action"}`).slice(0, 80);
    const path = `${runDir}/pandacode-${sanitizeSessionName(runtime || "runtime")}-${session}.report.json`;
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, JSON.stringify(report, null, 2));
    return path;
  } catch (error) {
    // Don't fail the node over a debug artifact, but leave a diagnostic instead
    // of dropping the raw report silently.
    emit({ type: "panda_raw_report_error", runtime, action, message: String(error?.message ?? error) });
    return "";
  }
}

function compactPandaCommand(command) {
  if (!command || typeof command !== "object" || Array.isArray(command)) {
    return command ?? null;
  }
  return {
    ok: command.ok,
    action: command.action,
    exit_code: command.exit_code,
    log_path: firstText(command.summary?.log_path),
    last_agent_message: truncateText(firstText(command.summary?.last_agent_message), 1200),
    stdout_tail: truncateText(firstText(command.stdout_tail, command.stdout), 600, "tail"),
    stderr_tail: truncateText(firstText(command.stderr_tail, command.stderr), 600, "tail"),
    summary: compactPandaSummary(command.summary),
    error: compactPandaError(command.error)
  };
}

function compactPandaSummary(summary) {
  if (!summary || typeof summary !== "object" || Array.isArray(summary)) {
    return summary ?? null;
  }
  return {
    ok: summary.ok,
    status: summary.status,
    current_phase: summary.current_phase,
    run_id: summary.run_id,
    thread_id: summary.thread_id,
    thread_path: summary.thread_path,
    turn_id: summary.turn_id,
    log_path: summary.log_path,
    last_agent_message: truncateText(firstText(summary.last_agent_message), 4000),
    counts: compactPandaCounts(summary.counts),
    usage: summary.usage ?? null,
    errors: compactPandaCount(summary.errors),
    warnings: compactPandaCount(summary.warnings)
  };
}

function compactPandaRecord(record) {
  if (!record || typeof record !== "object" || Array.isArray(record)) {
    return null;
  }
  return {
    runtime: record.runtime,
    session: record.session,
    driver: record.driver,
    workspace: record.workspace,
    run_id: record.run_id,
    thread_id: record.thread_id,
    thread_path: record.thread_path,
    tmux_name: record.tmux_name,
    model: record.model,
    effort: record.effort,
    permission: record.permission,
    artifacts: compactPandaArtifacts(record.artifacts),
    created_ms: record.created_ms,
    updated_ms: record.updated_ms
  };
}

function compactPandaNodeSummary(summary, { start, execute }) {
  if (!summary && !start && !execute) {
    return null;
  }
  return pruneEmpty({
    ok: summary?.ok ?? execute?.ok ?? start?.ok,
    status: summary?.status,
    current_phase: summary?.current_phase,
    counts: compactPandaCounts(summary?.counts),
    run_id: summary?.run_id,
    thread_id: summary?.thread_id,
    turn_id: summary?.turn_id,
    log_path: firstText(summary?.log_path, execute?.log_path, start?.log_path),
    usage: summary?.usage,
    start: start ? compactPandaStepForSummary(start) : undefined,
    execute: execute ? compactPandaStepForSummary(execute) : undefined,
    errors: compactPandaCount(summary?.errors),
    warnings: compactPandaCount(summary?.warnings)
  });
}

function compactPandaStepForSummary(step) {
  return pruneEmpty({
    ok: step.ok,
    exit_code: step.exit_code,
    log_path: step.log_path,
    stderr_tail: step.stderr_tail
  });
}

function compactPandaCounts(counts) {
  if (!counts || typeof counts !== "object" || Array.isArray(counts)) {
    return undefined;
  }
  return pruneEmpty({
    agent_messages: compactPandaCount(counts.agent_messages),
    errors: compactPandaCount(counts.errors),
    plans: compactPandaCount(counts.plans),
    questions: compactPandaCount(counts.questions),
    warnings: compactPandaCount(counts.warnings)
  });
}

function compactPandaCount(value) {
  return Number.isFinite(Number(value)) ? Number(value) : undefined;
}

function compactPandaError(error) {
  if (!error || typeof error !== "object" || Array.isArray(error)) {
    return null;
  }
  return pruneEmpty({
    category: error.category,
    message: truncateText(firstText(error.message), 1000),
    retryable: error.retryable,
    next_action: truncateText(firstText(error.next_action, error.nextAction), 1000),
    retry_prompt: truncateText(firstText(error.retry_prompt, error.retryPrompt), 1000)
  });
}

function compactPandaArtifacts(artifacts) {
  if (!artifacts || typeof artifacts !== "object" || Array.isArray(artifacts)) {
    return {};
  }
  const keys = [
    "prompt_file",
    "dispatch_prompt_file",
    "last_prompt_file",
    "last_dispatch_prompt_file",
    "transport",
    "last_transport",
    "log_dir",
    "log_path",
    "event_log",
    "debug_log",
    "session_socket",
    "transcript",
    "tmux_session",
    "raw_report"
  ];
  const compact = {};
  for (const key of keys) {
    if (artifacts[key] !== undefined) {
      compact[key] = artifacts[key];
    }
  }
  return compact;
}

function truncateText(value, limit, mode = "head") {
  const text = String(value || "");
  if (text.length <= limit) {
    return text;
  }
  if (mode === "tail") {
    return text.slice(-limit);
  }
  return `${text.slice(0, Math.max(0, limit - 18))}\n[truncated]`;
}

function pruneEmpty(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return value;
  }
  const pruned = {};
  for (const [key, item] of Object.entries(value)) {
    if (item === undefined || item === null || item === "") {
      continue;
    }
    if (typeof item === "object" && !Array.isArray(item)) {
      const nested = pruneEmpty(item);
      if (nested && Object.keys(nested).length > 0) {
        pruned[key] = nested;
      }
      continue;
    }
    pruned[key] = item;
  }
  return pruned;
}

// pandacode's claude/tmux runtime asks the agent to end its turn with a
// PANDACODE_DONE_<ms>_<pid> completion marker. The agent echoes it into its
// reply, and it reaches us via several capture paths (summary fields, the
// terminal tail, the hook event log). Strip the marker pattern from whatever we
// surface so it never leaks into the value a workflow node returns.
function stripCompletionMarker(text) {
  return String(text || "").replace(/\s*PANDACODE_DONE_\d+_\d+/g, "").trimEnd();
}

function readPandaCodeLastAssistantMessage(report) {
  // Strip BEFORE deciding: the tmux-scraped field is sometimes only the
  // completion marker, which strips to "". Treat that as empty and fall through
  // to the hook event log (whose Stop payload carries Claude Code's real
  // last_assistant_message) instead of returning a blank reply.
  const direct = stripCompletionMarker(firstText(
    report?.last_agent_message,
    report?.lastAgentMessage,
    report?.summary?.last_agent_message,
    report?.summary?.lastAgentMessage,
    // Bamboo (domestic-model) reports carry the final assistant text as the
    // summary's `summary` field rather than `last_agent_message`.
    report?.summary?.summary
  ));
  if (direct) {
    return direct;
  }
  const eventLog = firstText(
    report?.record?.artifacts?.event_log,
    report?.record?.artifacts?.eventLog,
    report?.artifacts?.event_log,
    report?.artifacts?.eventLog
  );
  if (!eventLog || !existsSync(eventLog)) {
    return "";
  }
  let lines;
  try {
    lines = readFileSync(eventLog, "utf8").trim().split(/\r?\n/).reverse();
  } catch {
    return "";
  }
  for (const line of lines) {
    if (!line.trim()) {
      continue;
    }
    let event;
    try {
      event = JSON.parse(line);
    } catch {
      // One malformed line must not abandon the scan — skip it and keep looking
      // for the last assistant message in the remaining (valid) lines.
      continue;
    }
    const message = stripCompletionMarker(firstText(
      event?.payload?.last_assistant_message,
      event?.payload?.lastAssistantMessage,
      event?.last_assistant_message,
      event?.lastAssistantMessage
    ));
    if (message) {
      return message;
    }
  }
  return "";
}

function writePromptFile(prompt, options) {
  mkdirSync(runDir, { recursive: true });
  const label = String(options.label || options.id || "codex")
    .replace(/[^a-zA-Z0-9_.-]+/g, "-")
    .replace(/^-+|-+$/g, "") || "codex";
  const file = `${runDir}/${label}-${Date.now()}.prompt.md`;
  writeFileSync(file, String(prompt));
  return file;
}

function parseInput(raw) {
  if (!raw) {
    return null;
  }
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

async function main() {
  if (!scriptPath) {
    throw new Error("ODW_SCRIPT_PATH is required");
  }
  const workflowInput = parseInput(process.env.ODW_INPUT || "");
  globalThis.args = workflowInput;
  globalThis.input = workflowInput;
  const budgetTotalRaw = Number(workflowInput?.budget?.total);
  state.budget ??= {
    total: Number.isFinite(budgetTotalRaw) && budgetTotalRaw > 0 ? budgetTotalRaw : null,
    spent: 0
  };
  const module = prepareWorkflowModule(scriptPath, workflowInput);
  const workflow = module.workflow;
  if (typeof workflow !== "function") {
    throw new Error(`workflow script must export a default function or use Dynamic Workflow-compatible top-level code: ${scriptPath}`);
  }
  const name = module.meta?.name || basename(scriptPath);
  workflowPhases = Array.isArray(module.meta?.phases) ? module.meta.phases : [];
  const whenToUse = module.meta?.whenToUse ?? null;
  state.workflow = { name, script: scriptPath, backend, resumeFrom, whenToUse };
  saveState();
  emit({ type: "workflow_start", name, script: scriptPath, backend, resumeFrom, whenToUse });
  const result = await workflow(workflowInput);
  // A workflow that returns a non-JSON-serializable value (circular reference,
  // BigInt, function) would otherwise crash the runner inside saveState()/emit()
  // with an opaque stringify TypeError surfaced only as "exited with status 1".
  // Turn it into a clean, structured failure the caller can read.
  let safeResult = result;
  try {
    JSON.stringify(result);
  } catch (error) {
    safeResult = {
      ok: false,
      error: {
        category: "result_not_serializable",
        message: `workflow "${name}" returned a non-JSON-serializable value: ${String(error?.message ?? error)}`
      }
    };
  }
  state.result = safeResult;
  state.completedAt = new Date().toISOString();
  saveState();
  emit({ type: "workflow_done", name, result: safeResult });
}

function prepareWorkflowModule(path, workflowInput) {
  const source = readFileSync(path, "utf8");
  const code = workflowSandboxSource(path, source);
  const context = vm.createContext(workflowSandboxGlobals(workflowInput), {
    name: "odw-workflow",
    codeGeneration: { strings: false, wasm: false }
  });
  vm.runInContext(code, context, {
    filename: path,
    timeout: 1_000,
    displayErrors: true
  });
  return {
    meta: context.__odwMeta,
    workflow: context.__odwWorkflow
  };
}

// Determinism guards: workflow scripts must be replayable on resume, so the
// non-deterministic clock/RNG entry points throw INSIDE the script sandbox only.
// Runner internals keep using the real globalThis.Date / Math (never touched here).
function scriptDeterminismGuards() {
  const RealDate = Date;
  function GuardedDate(...args) {
    if (!(this instanceof GuardedDate)) {
      throw new Error("Date() is not allowed in ODW workflow scripts (non-deterministic, breaks resume); pass an explicit timestamp to new Date(ts)");
    }
    if (args.length === 0) {
      throw new Error("argless new Date() is not allowed in ODW workflow scripts (non-deterministic, breaks resume); pass an explicit timestamp");
    }
    // Reflect.construct with GuardedDate as new.target makes the instance's
    // prototype (and thus .constructor) resolve to GuardedDate, not the real
    // Date — closing the `new Date(ts).constructor.now()` guard bypass.
    return Reflect.construct(RealDate, args, GuardedDate);
  }
  // Own prototype that inherits Date's methods but whose .constructor is the
  // guarded constructor (so it cannot hand back the real Date).
  GuardedDate.prototype = Object.create(RealDate.prototype);
  GuardedDate.prototype.constructor = GuardedDate;
  GuardedDate.UTC = RealDate.UTC;
  GuardedDate.parse = RealDate.parse;
  GuardedDate.now = () => {
    throw new Error("Date.now() is not allowed in ODW workflow scripts (non-deterministic, breaks resume)");
  };
  const GuardedMath = {};
  for (const key of Object.getOwnPropertyNames(Math)) {
    GuardedMath[key] = Math[key];
  }
  GuardedMath.random = () => {
    throw new Error("Math.random() is not allowed in ODW workflow scripts (non-deterministic, breaks resume)");
  };
  return { Date: GuardedDate, Math: GuardedMath };
}

function workflowSandboxGlobals(workflowInput) {
  const guards = scriptDeterminismGuards();
  return {
    args: workflowInput,
    input: workflowInput,
    console,
    log: globalThis.log,
    phase: globalThis.phase,
    agent: globalThis.agent,
    parallel: globalThis.parallel,
    fanout: globalThis.fanout,
    pipeline: globalThis.pipeline,
    checkpoint: globalThis.checkpoint,
    promptSlot: globalThis.promptSlot,
    budget: globalThis.budget,
    odw: globalThis.odw,
    pandacode: globalThis.pandacode,
    applyWorktreeDiff: globalThis.applyWorktreeDiff,
    applyWorktreeDiffs: globalThis.applyWorktreeDiffs,
    reviewWorktreeDiffs: globalThis.reviewWorktreeDiffs,
    captureMainWorktreeSnapshot: globalThis.captureMainWorktreeSnapshot,
    assertMainWorktreeUnchanged: globalThis.assertMainWorktreeUnchanged,
    restoreMainWorktreeSnapshot: globalThis.restoreMainWorktreeSnapshot,
    workflow: globalThis.workflow,
    setTimeout,
    clearTimeout,
    Date: guards.Date,
    Math: guards.Math
  };
}

function workflowSandboxSource(path, source) {
  assertWorkflowSourceSafe(path, source);
  if (/\bexport\s+default\b/.test(source) || /\bexport\s+(async\s+)?function\s+workflow\b/.test(source)) {
    return `${rewriteModuleExports(source)}
globalThis.__odwMeta = typeof meta === "undefined" ? { name: ${JSON.stringify(basename(path))} } : meta;
globalThis.__odwWorkflow = typeof __odwDefault === "function"
  ? __odwDefault
  : (typeof workflow === "function" ? workflow : null);
`;
  }

  const extracted = extractMeta(source);
  const meta = extracted.meta
    ? extracted.meta.replace(/\bexport\s+const\s+meta\b/, "const meta")
    : `const meta = { name: ${JSON.stringify(basename(path))} };`;
  return `${extracted.prelude || ""}
${meta}
globalThis.__odwMeta = meta;
globalThis.__odwWorkflow = async function __odwEntry(__odwInput) {
${extracted.body}
};
`;
}

function rewriteModuleExports(source) {
  return source
    .replace(/\bexport\s+const\s+meta\b/, "const meta")
    .replace(/\bexport\s+default\s+async\s+function(?:\s+[A-Za-z_$][\w$]*)?\s*\(/, "async function __odwDefault(")
    .replace(/\bexport\s+default\s+function(?:\s+[A-Za-z_$][\w$]*)?\s*\(/, "function __odwDefault(")
    .replace(/\bexport\s+async\s+function\s+workflow\s*\(/, "async function workflow(")
    .replace(/\bexport\s+function\s+workflow\s*\(/, "function workflow(");
}

function assertWorkflowSourceSafe(path, source) {
  const disallowed = [
    [/^\s*import\s.+from\s+["'][^"']+["'];?/m, "static import"],
    [/^\s*import\s+["'][^"']+["'];?/m, "static import"],
    [/\bimport\s*\(/, "dynamic import"],
    [/\brequire\s*\(/, "require"],
    [/\beval\s*\(/, "eval"],
    [/\bFunction\s*\(/, "Function constructor"]
  ];
  for (const [pattern, label] of disallowed) {
    if (pattern.test(source)) {
      throw new Error(`workflow script ${path} uses disallowed ${label}; ODW workflow scripts may only orchestrate agents`);
    }
  }
}

function extractMeta(source) {
  const marker = "export const meta";
  const start = source.indexOf(marker);
  if (start === -1) {
    return { prelude: "", meta: null, body: source };
  }
  const equals = source.indexOf("=", start);
  if (equals === -1) {
    return { prelude: "", meta: null, body: source };
  }
  let quote = null;
  let escaped = false;
  let braceDepth = 0;
  let bracketDepth = 0;
  let parenDepth = 0;
  for (let i = equals + 1; i < source.length; i += 1) {
    const ch = source[i];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === quote) {
        quote = null;
      }
      continue;
    }
    if (ch === "\"" || ch === "'" || ch === "`") {
      quote = ch;
      continue;
    }
    if (ch === "{") braceDepth += 1;
    if (ch === "}") braceDepth -= 1;
    if (ch === "[") bracketDepth += 1;
    if (ch === "]") bracketDepth -= 1;
    if (ch === "(") parenDepth += 1;
    if (ch === ")") parenDepth -= 1;
    if (ch === "}" && braceDepth === 0 && bracketDepth === 0 && parenDepth === 0) {
      let end = i + 1;
      while (end < source.length && /\s/.test(source[end])) {
        end += 1;
      }
      if (source[end] === ";") {
        end += 1;
      }
      return {
        prelude: source.slice(0, start),
        meta: source.slice(start, end),
        body: source.slice(end)
      };
    }
    if (ch === ";" && braceDepth === 0 && bracketDepth === 0 && parenDepth === 0) {
      return {
        prelude: source.slice(0, start),
        meta: source.slice(start, i + 1),
        body: source.slice(i + 1)
      };
    }
  }
  return { prelude: "", meta: null, body: source };
}

main().catch((error) => {
  const message = String(error?.stack || error?.message || error);
  // Persist the failure so `runs show` / a later inspection can report WHY a run
  // failed without re-reading the raw event tail.
  try {
    state.error = { message: String(error?.message || error) };
    state.failedAt = new Date().toISOString();
    saveState();
  } catch {
    /* best-effort: never mask the original failure */
  }
  emit({ type: "workflow_error", message });
  process.exitCode = 1;
});
