// Open Dynamic Workflow script API.
//
// This file documents the Dynamic Workflow-compatible script shape for `odw exec`.
// Scripts normally export literal `meta`, then run top-level async workflow code.
// ODW also accepts default async function exports for compatibility.

export interface WorkflowMeta {
  name: string;
  description?: string;
  /** When this workflow should be used (shown in the saved-workflow list). */
  whenToUse?: string;
  /**
   * Phase declarations. `model` lets a phase set a default model that its
   * agents inherit when they do not pass their own `options.model`.
   */
  phases?: Array<{ title: string; detail?: string; model?: string }>;
  agents?: string[];
  schemas?: string[];
  promptSlots?: string[];
}

export interface AgentOptions {
  id?: string;
  nodeId?: string;
  label?: string;
  phase?: string;
  /**
   * Optional author-defined routing/tag value. No fixed type enum is required.
   */
  agentType?: string;
  nodeType?: string;
  runtime?: "claude" | "codex" | "bamboo" | string;
  /**
   * PandaCode Bamboo provider, for example deepseek, xiaomi, kimi, zhipu,
   * minimax, qwen, or stepfun. Passing provider without runtime selects Bamboo.
   */
  provider?: string;
  bambooProvider?: string;
  model?: "inherit" | "haiku" | "sonnet" | "opus" | string;
  action?: "plan" | "start" | "execute" | "read" | "answer" | string;
  codexAction?: "plan" | "start" | "execute" | "read" | "answer" | string;
  /**
   * Run this node's executor in a throwaway git worktree branched from cwd, so
   * agents that mutate files in parallel do not conflict. The worktree is
   * removed when the node finishes (success, error, or timeout); the agent's
   * changes are returned in `result.worktree` as a diff. Requires cwd to be a
   * git repository. NOTE: a worktree only contains COMMITTED files, so commit or
   * stage any input files (specs, fixtures) the agent must read before running.
   */
  isolation?: "worktree";
  /**
   * Mock-backend only: synthetic total token count this node reports, so budget
   * loops and the budget ceiling can be exercised without spending real tokens.
   */
  mockTokens?: number;
  /**
   * Mock-backend only: relative filename to write into the node's worktree, so
   * the worktree diff-capture path is testable without a real executor.
   */
  mockWriteFile?: string;
  runId?: string;
  pick?: string;
  effort?: string;
  timeout?: string;
  sandbox?: string;
  approvalPolicy?: string;
  /**
   * Codex permission mode. Defaults to "max" (full access) because a coding node
   * usually needs the network and broad access to install dependencies and run
   * tests. Set "limited" to confine the node to its working directory with NO
   * network (workspace-write) — good for reviewing/analysing code, but it blocks
   * dependency installs (npm/pip/cargo). Codex runtime only.
   */
  permission?: "limited" | "max";
  /**
   * Optional final-response schema. The node still performs its normal work
   * first; this schema only constrains the final assistant response returned
   * to the workflow runner.
   */
  schema?: unknown;
  schemaDescription?: string;
  outputDescription?: string;
  finalResponseDescription?: string;
  maxAttempts?: number;
  retry?: {
    maxAttempts?: number;
    retryWhen?: string;
    backoff?: "none" | "linear" | "exponential";
  };
}

export interface ParallelNode extends AgentOptions {
  prompt?: string;
  input?: string;
}

export interface ParallelOptions {
  id?: string;
  label?: string;
  phase?: string;
  max?: number;
  concurrency?: number;
}

export interface WorkflowBudget {
  total: number | null;
  /**
   * Sum of each node's TOTAL tokens (input + output + cache + reasoning), not
   * output-only as in the built-in tool. Coding-agent nodes are input-dominated
   * (a trivial node can be ~19k total / <300 output), so budgets here should be
   * sized in total tokens — a loop ported from the built-in exhausts far sooner.
   */
  spent(): number;
  /** Remaining tokens, or `Infinity` when no `total` is set (guard on `total`). */
  remaining(): number;
}

export type OdwErrorCategory =
  | "codexctl_not_found"
  | "codexctl_auth"
  | "codexctl_rate_limit"
  | "codexctl_network"
  | "codexctl_permission"
  | "codexctl_model"
  | "codexctl_input"
  | "codexctl_failed"
  | "workflow_agent_failed"
  | "schema_mismatch"
  | "verification_failed"
  | "unknown";

export interface OdwErrorFeedback {
  ok: false;
  origin: {
    phase: string;
    agent: string;
    backend?: "codexctl" | "claude-code" | "shell" | string;
    attempt?: number;
  };
  error: {
    category: OdwErrorCategory;
    message: string;
    command?: string;
    exit_code?: number | null;
    stdout_tail?: string;
    stderr_tail?: string;
  };
  feedback: {
    retryable: boolean;
    user_message: string;
    next_action: string;
    retry_prompt?: string;
    required_change?: string;
  };
}

export declare function phase(title: string, detail?: string): void;

export declare const args: unknown;
export declare const input: unknown;
export declare const budget: WorkflowBudget;

/** Read-only metadata about the current run, exposed to the workflow. */
export declare const odw: {
  backend: "mock" | "pandacode" | string;
  runId: string;
  runDir: string;
  statePath: string;
  resumeFrom: string | null;
};

export declare function log(...args: unknown[]): void;

export declare function checkpoint(name: string, value?: unknown): void;

export declare function promptSlot(
  name: string,
  context?: Record<string, unknown>,
  suggested?: string
): string;

/**
 * Run one executor node and return its result.
 *
 * Return shape (built-in Workflow parity):
 * - No `schema`: returns the executor's final assistant message as a STRING.
 *   If the node ran with `isolation:"worktree"` and captured changes, returns a
 *   lean `{ text, worktree }` instead so the diff survives. The verbose raw
 *   executor report (socket/thread/log paths) is recorded in the run journal,
 *   not returned.
 * - With `schema`: returns the validated structured object (the node still does
 *   its real work first; the schema only constrains the final response). A
 *   captured worktree diff is attached as `result.worktree`.
 * - On failure: returns the structured error-feedback object (`ok:false`).
 */
export declare function agent<T = unknown>(
  prompt: string,
  options?: AgentOptions
): Promise<T>;

export declare function parallel<T = unknown>(
  thunks: Array<(index: number) => Promise<T>>,
  options?: ParallelOptions
): Promise<T[]>;

export declare function fanout<TItem = unknown, TResult = unknown>(
  items: TItem[],
  mapper: (item: TItem, index: number) => Promise<TResult> | TResult,
  options?: ParallelOptions
): Promise<TResult[]>;

export declare function pipeline<TItem = unknown, TResult = unknown>(
  items: TItem[],
  ...stages: Array<(value: unknown, item: TItem, index: number) => Promise<unknown> | unknown>
): Promise<TResult[]>;

/**
 * Run a saved/sibling workflow inline as one step and return its result.
 * `nameOrRef` resolves to `.claude/workflows/<name>.js`, `odw-<name>.js`, or a
 * relative/absolute path. The sub-workflow shares this run's agent counter,
 * concurrency caps, 1000-agent backstop, budget, and state. Nesting is 1 level
 * only: a sub-workflow that itself calls `workflow()` throws.
 */
export declare function workflow<TResult = unknown>(
  nameOrRef: string,
  args?: unknown
): Promise<TResult>;

/**
 * Convenience namespace for dispatching to a PandaCode runtime. Equivalent to
 * `agent(prompt, { ...options, runtime })`. Bamboo provider nodes use
 * `pandacode.bamboo(prompt, { provider })` or
 * `agent(prompt, { runtime: "bamboo", provider })`.
 */
export declare const pandacode: {
  exec<T = unknown>(runtime: string, prompt: string, options?: AgentOptions): Promise<T>;
  claude<T = unknown>(prompt: string, options?: AgentOptions): Promise<T>;
  codex<T = unknown>(prompt: string, options?: AgentOptions): Promise<T>;
  bamboo<T = unknown>(prompt: string, options?: AgentOptions): Promise<T>;
};

export type WorkflowEntrypoint<TInput = unknown, TResult = unknown> = (
  input: TInput
) => Promise<TResult>;
