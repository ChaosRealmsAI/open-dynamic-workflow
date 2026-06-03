#!/usr/bin/env node
// ODW parity self-test: odw verifies its own runtime by running `odw exec`
// against crafted workflows and asserting on the built-in Workflow parity
// features (concurrency, determinism guard, worktree isolation, budget
// accounting, nested workflow, per-phase model / whenToUse).
//
// Usage:
//   node scripts/selftest.mjs            # uses ./target/debug/odw
//   ODW=/path/to/odw node scripts/selftest.mjs
//
// Exits 0 only if every assertion passes. Deterministic and token-free
// (mock backend). Worktree tests require this repo to be a git checkout.

import { spawnSync } from "node:child_process";
import {
  writeFileSync, readFileSync, existsSync, mkdtempSync, mkdirSync, rmSync, readdirSync, chmodSync
} from "node:fs";
import { tmpdir, cpus } from "node:os";
import { join, resolve } from "node:path";

// Absolute so it still resolves when a test runs odw from another cwd.
const ODW = resolve(process.env.ODW || "./target/debug/odw");
const REPO = process.cwd();
const EXPECTED_MAX = Math.max(1, Math.min(16, cpus().length - 2));

let seq = 0;
const tmpRoot = mkdtempSync(join(tmpdir(), "odw-selftest-"));
function writeScript(src) {
  const p = join(tmpRoot, `wf-${seq++}.js`);
  writeFileSync(p, src);
  return p;
}

// Write an executable script (e.g. a fake pandacode bin) and return its path.
function writeExec(name, src) {
  const p = join(tmpRoot, name);
  writeFileSync(p, src);
  chmodSync(p, 0o755);
  return p;
}

// Run `odw exec` for a workflow source. Returns { code, out, events, state }.
// `backend`/`env`/`pandacodeBin` let failure-path tests drive a fake executor.
function run(src, { input = {}, cwd = REPO, resume = null, scriptPath = null, json = false, backend = "mock", env = {}, pandacodeBin = null } = {}) {
  const args = resume
    ? ["exec", "--resume", resume, "--backend", backend]
    : ["exec", "--script", scriptPath || writeScript(src), "--input", JSON.stringify(input), "--backend", backend];
  if (json) {
    args.push("--json");
  }
  const childEnv = { ...process.env, ...env };
  if (pandacodeBin) {
    childEnv.ODW_PANDACODE_BIN = pandacodeBin;
  }
  const r = spawnSync(ODW, args, { cwd, encoding: "utf8", env: childEnv });
  const out = (r.stdout || "") + (r.stderr || "");
  const runId = (out.match(/run_id=(\S+)/) || [])[1] || null;
  const runDir = runId ? join(cwd, ".odw", "runs", runId) : null;
  let events = [];
  let state = {};
  if (runDir && existsSync(join(runDir, "events.jsonl"))) {
    events = readFileSync(join(runDir, "events.jsonl"), "utf8")
      .split(/\r?\n/).filter(Boolean)
      .map((l) => { try { return JSON.parse(l); } catch { return null; } })
      .filter(Boolean)
      .map((e) => (e.type === "script_stream" && e.raw ? e.raw : e));
  }
  if (runDir && existsSync(join(runDir, "state.json"))) {
    try { state = JSON.parse(readFileSync(join(runDir, "state.json"), "utf8")); } catch { /* ignore */ }
  }
  return { code: r.status ?? 1, out, events, state, runId, runDir };
}

function runOdw(args, { cwd = REPO, env = {}, pandacodeBin = null } = {}) {
  const childEnv = { ...process.env, ...env };
  if (pandacodeBin) {
    childEnv.ODW_PANDACODE_BIN = pandacodeBin;
  }
  const r = spawnSync(ODW, args, { cwd, encoding: "utf8", env: childEnv });
  return { code: r.status ?? 1, out: (r.stdout || "") + (r.stderr || "") };
}

const ev = (events, type) => events.filter((e) => e && e.type === type);
const logLine = (out, re) => (out.match(re) || [])[1];

// Git worktrees odw left registered (path under an .odw run's worktrees/ dir).
// Filtering by that path keeps the assertion robust to unrelated worktrees
// (e.g. a review checkout) that may exist in this repo during development.
function odwWorktreeLeftovers() {
  const wl = spawnSync("git", ["worktree", "list"], { cwd: REPO, encoding: "utf8" }).stdout || "";
  return wl.trim().split(/\r?\n/).filter((l) => /[/\\]worktrees[/\\]/.test(l));
}

const cases = [];
const test = (name, fn) => cases.push({ name, fn });
function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

// 1. Cores-aware concurrency cap -------------------------------------------
test("concurrency: parallel caps at min(16, cores-2)", () => {
  const r = run(`export const meta={name:"c"};
phase("P","");
await parallel(Array.from({length:12},(_,i)=>()=>agent("n"+i,{label:"n"+i})));
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  const ps = ev(r.events, "parallel_start")[0];
  assert(ps, "no parallel_start event");
  assert(ps.max === EXPECTED_MAX, `parallel max=${ps.max}, expected ${EXPECTED_MAX}`);
});

test("concurrency: per-call options.max overrides downward", () => {
  const r = run(`export const meta={name:"c2"};
phase("P","");
await parallel(Array.from({length:8},(_,i)=>()=>agent("n"+i,{label:"n"+i})),{max:2});
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  const ps = ev(r.events, "parallel_start")[0];
  assert(ps.max === 2, `expected max=2, got ${ps.max}`);
});

// 2. Determinism guard ------------------------------------------------------
for (const [label, expr, needle] of [
  ["Date.now()", "Date.now()", "Date.now() is not allowed"],
  ["argless new Date()", "new Date()", "argless new Date() is not allowed"],
  ["Math.random()", "Math.random()", "Math.random() is not allowed"]
]) {
  test(`determinism: ${label} throws in script`, () => {
    const r = run(`export const meta={name:"d"};
phase("P","");
const x = ${expr};
return {ok:true,x};`);
    assert(r.code !== 0, `expected failure but run succeeded`);
    assert(r.out.includes(needle), `expected "${needle}" in output, got: ${r.out.slice(-300)}`);
  });
}

test("determinism: deterministic Date(ts)/Math.* still work", () => {
  const r = run(`export const meta={name:"d2"};
phase("P","");
const a = Math.floor(3.7) + Math.max(1,2);
const iso = new Date(0).toISOString();
log("OKVAL a="+a+" iso="+iso);
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/OKVAL a=5 iso=1970-01-01T00:00:00.000Z/.test(r.out), `deterministic forms broke: ${r.out.slice(-300)}`);
});

test("determinism: new Date(ts).constructor.now() bypass is blocked", () => {
  const r = run(`export const meta={name:"db"};
phase("P","");
const leaked = new Date(0).constructor.now();
return {ok:true, leaked};`);
  assert(r.code !== 0, `constructor.now() bypass should throw, got code ${r.code}: ${r.out.slice(-200)}`);
  assert(/Date\.now\(\) is not allowed/.test(r.out), `expected Date.now block: ${r.out.slice(-300)}`);
});

// 3. Worktree isolation -----------------------------------------------------
test("worktree: create + cleanup, no leftovers", () => {
  const r = run(`export const meta={name:"w"};
phase("P","");
await parallel([0,1,2].map(i=>()=>agent("w"+i,{id:"w"+i,label:"w"+i,isolation:"worktree"})));
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(ev(r.events, "worktree_start").length === 3, `expected 3 worktree_start, got ${ev(r.events,"worktree_start").length}`);
  assert(ev(r.events, "worktree_done").length === 3, `expected 3 worktree_done`);
  const wtDir = join(r.runDir, "worktrees");
  const leftovers = existsSync(wtDir) ? readdirSync(wtDir) : [];
  assert(leftovers.length === 0, `worktree leftovers: ${leftovers.join(",")}`);
  assert(odwWorktreeLeftovers().length === 0, `stale git worktrees:\n${odwWorktreeLeftovers().join("\n")}`);
});

test("worktree: non-git cwd yields clear error", () => {
  const dir = mkdtempSync(join(tmpdir(), "odw-nogit-"));
  const sp = join(dir, "w.js");
  writeFileSync(sp, `export const meta={name:"w"};
phase("P","");
await agent("x",{label:"x",isolation:"worktree"});
return {ok:true};`);
  const r = run(null, { cwd: dir, scriptPath: sp });
  const hay = r.out + JSON.stringify(r.events);
  assert(/requires .* to be a git repository/.test(hay), `expected git-repo error, got: ${(r.out || "(empty)").slice(-300)}`);
  rmSync(dir, { recursive: true, force: true });
});

test("worktree: unchanged node -> result.worktree.changed=false", () => {
  const r = run(`export const meta={name:"wu"};
phase("P","");
const a = await agent("noop",{id:"wu",label:"wu",isolation:"worktree"});
log("WU changed="+a.worktree?.changed+" files="+(a.worktree?.files||[]).length);
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/WU changed=false files=0/.test(r.out), `expected unchanged capture: ${r.out.slice(-300)}`);
});

test("worktree: changed node -> captures files + diff (no data loss), dir removed", () => {
  const r = run(`export const meta={name:"wc"};
phase("P","");
const a = await agent("writes",{id:"wc",label:"wc",isolation:"worktree",mockWriteFile:"selftest_change.txt"});
log("WC changed="+a.worktree?.changed+" files="+(a.worktree?.files||[]).join(",")+" diffhas="+/selftest_change/.test(a.worktree?.diff||""));
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/WC changed=true files=selftest_change.txt diffhas=true/.test(r.out), `expected change capture: ${r.out.slice(-300)}`);
  assert(odwWorktreeLeftovers().length === 0, `dir not removed after capture:\n${odwWorktreeLeftovers().join("\n")}`);
});

test("worktree: unchanged EXECUTOR node keeps {text, worktree:{changed:false}} (consistent shape)", () => {
  // Real executor reports collapse through leanAgentResult; an unchanged worktree
  // node must keep its worktree object instead of decaying to a bare string, so
  // worktree nodes always expose `result.worktree.changed`. mockAgentText makes
  // the mock return an executor-report envelope, exercising that path token-free.
  const r = run(`export const meta={name:"wue"};
phase("P","");
const a = await agent("noop", { id:"wue", label:"wue", isolation:"worktree", mockAgentText:"did nothing" });
log("WUE type="+typeof a+" changed="+(a&&a.worktree&&a.worktree.changed)+" text="+(a&&a.text));
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/WUE type=object changed=false text=did nothing/.test(r.out),
    `unchanged executor worktree node must keep its worktree object: ${r.out.slice(-300)}`);
});

// 4. Budget accounting ------------------------------------------------------
test("budget: accrual + remaining + hard ceiling", () => {
  const r = run(`export const meta={name:"b"};
phase("P","");
log("S0 "+budget.spent()+" "+budget.remaining());
await agent("a1",{label:"a1",mockTokens:1000});
log("S1 "+budget.spent()+" "+budget.remaining());
await agent("a2",{label:"a2",mockTokens:1000});
log("S2 "+budget.spent()+" "+budget.remaining());
await agent("a3",{label:"a3",mockTokens:1000});
return {ok:true};`, { input: { budget: { total: 1500 } } });
  assert(r.code !== 0, "expected ceiling to throw on a3");
  assert(/budget exhausted: spent 2000 >= total 1500/.test(r.out), `expected exhaustion msg, got: ${r.out.slice(-300)}`);
  assert(/S0 0 1500/.test(r.out), "S0 wrong");
  assert(/S1 1000 500/.test(r.out), "S1 wrong (accrual)");
  assert(/S2 2000 0/.test(r.out), "S2 wrong (remaining clamps at 0)");
});

test("budget: resume does not double-count cached nodes", () => {
  const first = run(`export const meta={name:"br"};
phase("P","");
await agent("a1",{label:"a1",mockTokens:1000});
checkpoint("c1",{});
await agent("a2",{label:"a2",mockTokens:1000});
log("FINAL "+budget.spent());
return {ok:true};`, { input: { budget: { total: 100000 } } });
  assert(first.code === 0, `first run failed: ${first.out.slice(-300)}`);
  assert(logLine(first.out, /FINAL (\d+)/) === "2000", `first spent != 2000`);
  const resumed = run(null, { resume: first.runId });
  assert(resumed.code === 0, `resume failed: ${resumed.out.slice(-300)}`);
  assert(/skip .* cached=true/.test(resumed.out), "expected cached skips on resume");
  assert(logLine(resumed.out, /FINAL (\d+)/) === "2000", `resume double-counted: ${logLine(resumed.out,/FINAL (\d+)/)}`);
});

test("budget: no-token node flags approx floor", () => {
  const r = run(`export const meta={name:"ba"};
phase("P","");
await agent("a1",{label:"a1"});
return {ok:true};`, { input: { budget: { total: 100000 } } });
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(r.state.budget && r.state.budget.approx === true, `expected budget.approx=true, got ${JSON.stringify(r.state.budget)}`);
});

// 5. Nested workflow --------------------------------------------------------
test("workflow(): nested shares budget/counter + passes args", () => {
  const child = writeScript(`export const meta={name:"child"};
phase("C","");
await agent("c1",{label:"c1",mockTokens:500});
await agent("c2",{label:"c2",mockTokens:500});
return {ok:true,childSpent:budget.spent(),gotArgs:args?.from??null};`);
  const r = run(`export const meta={name:"parent"};
phase("P","");
await agent("p1",{label:"p1",mockTokens:1000});
const sub = await workflow(${JSON.stringify(child)},{from:"parent"});
log("CHILD spent="+budget.spent()+" childSaw="+sub?.childSpent+" args="+sub?.gotArgs);
return {ok:true};`, { input: { budget: { total: 100000 } } });
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/CHILD spent=2000 childSaw=2000 args=parent/.test(r.out), `nested sharing/args wrong: ${r.out.slice(-300)}`);
});

test("workflow(): name resolves via .claude/workflows", () => {
  const wfDir = join(REPO, ".claude", "workflows");
  mkdirSync(wfDir, { recursive: true }); // clean checkouts have no .claude/ yet
  const named = join(wfDir, "odw-selftest-childtmp.js");
  writeFileSync(named, `export const meta={name:"childtmp"};
await agent("c1",{label:"c1",mockTokens:300});
return {ok:true,childSpent:budget.spent()};`);
  try {
    const r = run(`export const meta={name:"pn"};
const sub = await workflow("selftest-childtmp",{});
log("BYNAME "+sub?.childSpent);
return {ok:true};`, { input: { budget: { total: 100000 } } });
    assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
    assert(/BYNAME 300/.test(r.out), `name resolution failed: ${r.out.slice(-300)}`);
  } finally {
    rmSync(named, { force: true });
  }
});

test("workflow(): nesting is 1 level only", () => {
  const leaf = writeScript(`export const meta={name:"leaf"};
return {ok:true};`);
  const child = writeScript(`export const meta={name:"badchild"};
await workflow(${JSON.stringify(leaf)},{});
return {ok:true};`);
  const r = run(`export const meta={name:"badparent"};
await workflow(${JSON.stringify(child)},{});
return {ok:true};`);
  assert(r.code !== 0, "expected 1-level violation to throw");
  assert(/nested workflow\(\) is 1-level only/.test(r.out), `expected 1-level error, got: ${r.out.slice(-300)}`);
});

// 6. meta.whenToUse + per-phase model --------------------------------------
test("meta.whenToUse surfaces in workflow_start", () => {
  const r = run(`export const meta={name:"meta",whenToUse:"SELFTEST_WHENTOUSE",phases:[{title:"Build",model:"opus"}]};
phase("Build","");
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  const ws = ev(r.events, "workflow_start")[0];
  assert(ws && ws.whenToUse === "SELFTEST_WHENTOUSE", `whenToUse missing: ${JSON.stringify(ws)}`);
});

test("schema: works WITHOUT schemaDescription (optional, matches built-in)", () => {
  const r = run(`export const meta={name:"sd"};
phase("P","");
const a = await agent("structured", { label:"a", schema:{ type:"object", properties:{ ok:{type:"boolean"} }, required:["ok"] } });
log("SD ok="+(a?.ok!==false));
return {ok:true};`);
  assert(r.code === 0, `schema without schemaDescription threw: ${r.out.slice(-300)}`);
  assert(/SD ok=true/.test(r.out), `schema-without-desc node did not run: ${r.out.slice(-300)}`);
  assert(!/schemaDescription/.test(r.out), `schemaDescription should no longer be required`);
});

test("schema: mismatch retries then collapses to a structured schema_mismatch failure", () => {
  const r = run(`export const meta={name:"sm"};
phase("P","");
const out = await agent("x",{ label:"x", maxAttempts:2, schema:{ title:"inline", type:"object", required:["definitelyMissing"] } });
log("SM ok="+(out?.ok)+" cat="+(out?.error?.category)+" issues="+(out?.error?.issues?.length||0));
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  const invalid = ev(r.events, "agent_schema_invalid");
  const retry = ev(r.events, "agent_retry");
  assert(invalid.length === 2, `expected 2 agent_schema_invalid, got ${invalid.length}`);
  assert(invalid[0].retryable === true && invalid[1].retryable === false, `retryable flags wrong: ${JSON.stringify(invalid.map((e) => e.retryable))}`);
  assert(retry.length === 1 && retry[0].reason === "schema_mismatch" && retry[0].nextAttempt === 2, `retry event wrong: ${JSON.stringify(retry[0])}`);
  assert(/SM ok=false cat=schema_mismatch issues=[1-9]/.test(r.out), `final schema_mismatch result wrong: ${r.out.slice(-300)}`);
});

test("schema: an unloadable schema fails fast and non-retryably", () => {
  // A typo'd/missing schema path is a config error, not a transient mismatch:
  // it must fail with a clear category and NOT burn retries (the file won't
  // appear on retry). Distinct from schema_mismatch, which is retryable.
  const r = run(`export const meta={name:"sle"};
phase("P","");
const out = await agent("x",{ label:"sle", retry:{maxAttempts:3}, schema:"does-not-exist-schema.json" });
log("SLE ok="+(out?.ok)+" cat="+(out?.error?.category)+" retryable="+(out?.error?.retryable));
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/SLE ok=false cat=schema_load_error retryable=false/.test(r.out), `unloadable schema result wrong: ${r.out.slice(-300)}`);
  assert(ev(r.events, "agent_retry").length === 0, `unloadable schema must not retry, saw ${ev(r.events, "agent_retry").length}`);
});

// 7. Key error/edge semantics (must match built-in) ------------------------
test("parallel: a thrown thunk -> null, batch never rejects", () => {
  const r = run(`export const meta={name:"pt"};
phase("P","");
const out = await parallel([
  () => agent("a",{label:"a",mockTokens:1}),
  () => { throw new Error("boom"); },
  () => agent("c",{label:"c",mockTokens:1})
]);
log("PT len="+out.length+" mid="+(out[1]===null?"null":"NOTNULL")+" ends="+(out[0]&&out[2]?"ok":"bad"));
return {ok:true};`);
  assert(r.code === 0, `parallel rejected the whole batch: ${r.out.slice(-300)}`);
  assert(/PT len=3 mid=null ends=ok/.test(r.out), `parallel error semantics wrong: ${r.out.slice(-300)}`);
});

test("parallel/pipeline: a failed (null) item makes *_done telemetry ok=false", () => {
  const r = run(`export const meta={name:"pf"};
phase("P","");
await parallel([ () => agent("a",{label:"a"}), () => { throw new Error("boom"); } ]);
await pipeline([1,2], (v)=>{ if(v===2) throw new Error("x"); return v; });
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-200)}`);
  const pd = ev(r.events, "parallel_done")[0];
  const pl = ev(r.events, "pipeline_done")[0];
  assert(pd && pd.ok === false, `parallel_done.ok must be false when an item throws, got ${JSON.stringify(pd)}`);
  assert(pl && pl.ok === false, `pipeline_done.ok must be false when an item throws, got ${JSON.stringify(pl)}`);
});

test("pipeline: a thrown stage -> that item null, others continue", () => {
  const r = run(`export const meta={name:"pp"};
phase("P","");
const out = await pipeline([1,2,3], (v)=>{ if(v===2) throw new Error("boom2"); return v*10; });
log("PP "+JSON.stringify(out));
return {ok:true};`);
  assert(r.code === 0, `pipeline rejected: ${r.out.slice(-300)}`);
  assert(/PP \[10,null,30\]/.test(r.out), `pipeline error semantics wrong: ${r.out.slice(-300)}`);
});

test("pipeline: each stage gets (prevResult, originalItem, index) — built-in parity", () => {
  // The built-in contract: later stages see the prior stage's result AND the
  // ORIGINAL item + its index (so you can label work without threading context
  // through stage 1's return). Lock that signature so a refactor can't break it.
  const r = run(`export const meta={name:"ppsig"};
phase("P","");
const out = await pipeline(["A","B"],
  (prev, item, index) => "s1:"+item+":"+index,
  (prev, item, index) => prev+"|item="+item+"|idx="+index
);
log("SIG "+JSON.stringify(out));
return {ok:true};`);
  assert(r.code === 0, `pipeline rejected: ${r.out.slice(-300)}`);
  assert(
    /SIG \["s1:A:0\|item=A\|idx=0","s1:B:1\|item=B\|idx=1"\]/.test(r.out),
    `stage signature wrong (expected (prev, originalItem, index)): ${r.out.slice(-300)}`
  );
});

test("budget: remaining() is Infinity when no total set", () => {
  const r = run(`export const meta={name:"bi2"};
log("REM="+budget.remaining());
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/REM=Infinity/.test(r.out), `expected Infinity, got: ${r.out.slice(-200)}`);
});

// 8. Caller experience: result return, --json, exit codes ------------------
test("exec: prints [result] <json> with the workflow return value", () => {
  const r = run(`export const meta={name:"r"};
phase("P","");
return { answer: 42, tag: "selftest-result" };`);
  assert(r.code === 0, `run failed: ${r.out.slice(-200)}`);
  assert(/\[result\] \{.*"answer":42.*"tag":"selftest-result".*\}/.test(r.out), `no [result] line: ${r.out.slice(-200)}`);
});

test("result: a non-serializable return is a clean failure, not an opaque crash", () => {
  // A circular return would crash JSON.stringify inside the runner; instead of an
  // opaque "exited with status 1", it must surface a structured failure.
  const r = run(`export const meta={name:"ns"};
phase("P","");
const o = { a: 1 }; o.self = o;
return o;`);
  assert(r.code !== 0, `non-serializable return must fail non-zero, got ${r.code}`);
  const done = ev(r.events, "workflow_done");
  assert(
    done.length === 1 && done[0].result?.error?.category === "result_not_serializable",
    `expected result_not_serializable, got ${JSON.stringify(done[0]?.result)}`
  );
});

test("exec: --json prints only the result object (no progress lines)", () => {
  const r = run(`export const meta={name:"rj"};
phase("P","");
log("noise");
return { only: "result", n: 7 };`, { json: true });
  assert(r.code === 0, `run failed: ${r.out.slice(-200)}`);
  const lines = r.out.trim().split(/\r?\n/).filter(Boolean);
  assert(lines.length === 1, `--json should print exactly one line, got ${lines.length}: ${r.out.slice(-200)}`);
  assert(JSON.parse(lines[0]).only === "result", `--json result wrong: ${lines[0]}`);
  assert(!/\[workflow\]|\[phase\]|\[log\]/.test(r.out), `--json leaked progress lines`);
});

test("exec: exits non-zero when workflow returns ok:false", () => {
  const ok = run(`export const meta={name:"x"};return { ok:true };`);
  assert(ok.code === 0, "ok:true should exit 0");
  const bad = run(`export const meta={name:"x"};return { ok:false, error:"boom" };`);
  assert(bad.code !== 0, `ok:false should exit non-zero, got ${bad.code}`);
  // --json still prints the failing result before exiting non-zero
  const badJson = run(`export const meta={name:"x"};return { ok:false, error:"boom" };`, { json: true });
  assert(badJson.code !== 0 && /"ok":false/.test(badJson.out), `--json ok:false should print result + exit non-zero`);
});

// 9. No-schema lean return (built-in parity): report -> text / {text,worktree} -
test("agent: no-schema executor report collapses to final-text string", () => {
  const r = run(`export const meta={name:"lean1"};
const t = await agent("hi", { mockAgentText: "HELLO-LEAN-MESSAGE" });
log("T type="+typeof t+" val="+JSON.stringify(t));
return { ok:true };`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/T type=string val="HELLO-LEAN-MESSAGE"/.test(r.out),
    `no-schema report should collapse to bare string, got: ${r.out.slice(-300)}`);
});

test("agent: no-schema + worktree returns lean {text, worktree}, no report noise", () => {
  const r = run(`export const meta={name:"lean2"};
const x = await agent("go", { mockAgentText:"WT-LEAN", isolation:"worktree", mockWriteFile:"lean_change.txt", label:"w" });
const keys = Object.keys(x).sort().join(",");
const leaked = ["adapter","run_id","thread_id","summary","backend","state"].filter(k => k in x);
log("LEAN keys="+keys+" text="+x.text+" changed="+x.worktree.changed+" files="+x.worktree.files.join("|")+" leaked="+leaked.join("|"));
return { ok:true };`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/LEAN keys=text,worktree text=WT-LEAN changed=true files=lean_change.txt leaked=/.test(r.out),
    `worktree lean shape wrong: ${r.out.slice(-300)}`);
});

test("agent: no-schema NON-executor (mock) result is not collapsed", () => {
  const r = run(`export const meta={name:"lean3"};
const m = await agent("plain", { label:"p" });
log("MOCK type="+typeof m+" backend="+(m&&m.backend)+" hasPreview="+(m&&"prompt_preview" in m));
return { ok:true };`);
  assert(r.code === 0, `run failed: ${r.out.slice(-300)}`);
  assert(/MOCK type=object backend=mock hasPreview=true/.test(r.out),
    `non-executor mock result must stay an object: ${r.out.slice(-300)}`);
});

// 10. pandacode failure surfacing (token-free via a fake executor bin) --------
// A fake `pandacode` that prints a JSON report and exits with a chosen code,
// selected by the FAKE_PANDA env var — lets us test odw's failure收口 without
// a real executor.
const FAKE_PANDA = `#!/usr/bin/env node
const s = process.env.FAKE_PANDA || "exit1_oktrue";
const args = process.argv.slice(2);
if (args[0] === "--version") {
  process.stdout.write("pandacode fake 0.0.0\\n");
  process.exit(0);
}
if (args[0] === "doctor") {
  process.stdout.write(JSON.stringify({
    ok: true,
    state: "checked",
    codex: { ok: true, runtime: "codex" },
    claude: { ok: false, runtime: "claude", missing: ["auth"] },
    bamboo: {
      ok: false,
      runtime: "bamboo",
      state: "configuration_needed",
      missing: ["api_key"],
      active: { provider: "deepseek", api_key_present: false },
      warnings: ["Set PANDACODE_BAMBOO_API_KEY before live runs."]
    }
  }) + "\\n");
  process.exit(0);
}
if (s === "argv") {
  const message = args.join(" ");
  process.stdout.write(JSON.stringify({
    ok: true,
    state: "completed",
    runtime: args[0] || "",
    last_agent_message: message,
    summary: { last_agent_message: message }
  }) + "\\n");
  process.exit(0);
}
if (s === "bamboo_usage") {
  process.stdout.write(JSON.stringify({
    ok: true,
    state: "completed",
    runtime: "bamboo",
    last_agent_message: "bamboo usage ok",
    summary: {
      last_agent_message: "bamboo usage ok",
      usage: { calls: 1, input_tokens: 100, output_tokens: 23, total_tokens: 123 }
    }
  }) + "\\n");
  process.exit(0);
}
if (s === "jsonl_final_report") {
  process.stdout.write(JSON.stringify({ type: "start", message: "EARLY_EVENT_MESSAGE" }) + "\\n");
  process.stdout.write(JSON.stringify({ type: "delta", last_agent_message: "EARLY_EVENT_MESSAGE" }) + "\\n");
  process.stdout.write(JSON.stringify({
    ok: true,
    state: "completed",
    runtime: args[0] || "",
    last_agent_message: "FINAL_JSONL_REPORT_MESSAGE",
    summary: { last_agent_message: "FINAL_JSONL_REPORT_MESSAGE" }
  }) + "\\n");
  process.exit(0);
}
const R = {
  exit1_oktrue: ['{"ok":true,"state":"completed","summary":{"ok":true},"last_agent_message":"all good"}', 1],
  exit1_nook:   ['{"state":"completed","summary":{},"last_agent_message":"done-ish"}', 1],
  okfalse:      ['{"ok":false,"state":"failed","error":{"category":"codexctl_rate_limit","message":"rate limited"}}', 0],
  bamboo_reply: ['{"ok":true,"state":"completed","runtime":"bamboo","summary":{"status":"completed","summary":"BAMBOO-REPLY-TEXT"}}', 0],
};
const [out, code] = R[s] || R.exit1_oktrue;
process.stdout.write(out + "\\n");
process.exit(code);
`;
const fakePanda = writeExec("fake-panda.mjs", FAKE_PANDA);
const FAILWF = `export const meta={name:"f"};
const r = await agent("x",{runtime:"codex",label:"n"});
return { ok: r?.ok !== false, nodeGotOkFalse: r?.ok === false, category: r?.error?.category };`;

test("pandacode: non-zero exit with ok:true/absent report is surfaced as failure (not swallowed)", () => {
  for (const scen of ["exit1_oktrue", "exit1_nook"]) {
    const r = run(FAILWF, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: scen } });
    assert(r.code !== 0, `[${scen}] non-zero exit must fail the workflow, got code ${r.code}: ${r.out.slice(-200)}`);
    assert(/"nodeGotOkFalse":true/.test(r.out), `[${scen}] node must be ok:false: ${r.out.slice(-200)}`);
  }
});

test("pandacode: structured ok:false report preserves error category + fails", () => {
  const r = run(FAILWF, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: "okfalse" } });
  assert(r.code !== 0, `ok:false must fail: ${r.out.slice(-200)}`);
  assert(/"category":"codexctl_rate_limit"/.test(r.out), `error category lost: ${r.out.slice(-200)}`);
});

test("pandacode: JSONL stdout selects final report instead of earlier events", () => {
  const wf = `export const meta={name:"jsonl"};
const r = await agent("x",{runtime:"codex",label:"jsonl-node",id:"jsonl-node"});
log("JSONL_RESULT="+r);
return { ok:true };`;
  const r = run(wf, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: "jsonl_final_report" } });
  assert(r.code === 0, `jsonl report run failed: ${r.out.slice(-300)}`);
  assert(/JSONL_RESULT=FINAL_JSONL_REPORT_MESSAGE/.test(r.out), `final report message not selected: ${r.out.slice(-500)}`);
  assert(!/JSONL_RESULT=EARLY_EVENT_MESSAGE/.test(r.out), `early event was selected as report: ${r.out.slice(-500)}`);
});

test("pandacode: Bamboo provider dispatch argv and helper are passed through", () => {
  const wf = `export const meta={name:"pb"};
const args = await agent("x",{runtime:"bamboo",provider:"deepseek",model:"deepseek-v4-pro",effort:"high",label:"bamboo-node",id:"bamboo-node"});
const helper = await pandacode.bamboo("y",{provider:"deepseek",label:"bamboo-helper",id:"bamboo-helper"});
log("PARGS="+args);
log("HARGS="+helper);
return { ok:true };`;
  const r = run(wf, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: "argv" } });
  assert(r.code === 0, `bamboo provider run failed: ${r.out.slice(-300)}`);
  assert(/PARGS=bamboo exec --provider deepseek\b/.test(r.out), `bamboo provider argv wrong: ${r.out.slice(-500)}`);
  assert(/HARGS=bamboo exec --provider deepseek\b/.test(r.out), `pandacode.bamboo helper argv wrong: ${r.out.slice(-500)}`);
  assert(/--model deepseek-v4-pro/.test(r.out), `model not passed: ${r.out.slice(-500)}`);
  assert(/--effort high/.test(r.out), `effort not passed: ${r.out.slice(-500)}`);
});

test("pandacode: provider on non-Bamboo runtime is a clear error", () => {
  const wf = `export const meta={name:"pbad"};
const r = await agent("x",{runtime:"codex",provider:"deepseek",label:"bad-provider",id:"bad-provider"});
log("ERR="+(r?.error?.message||""));
return { ok: r?.ok !== false };`;
  const r = run(wf, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: "argv" } });
  assert(r.code !== 0, `non-bamboo provider should fail: ${r.out.slice(-300)}`);
  assert(/provider is only supported for PandaCode Bamboo nodes; got runtime=codex/.test(r.out), `provider error unclear: ${r.out.slice(-500)}`);
});

test("mock: Bamboo provider agent returns normally", () => {
  const wf = `export const meta={name:"mb"};
const r = await agent("x",{runtime:"bamboo",provider:"deepseek",label:"mock-bamboo",id:"mock-bamboo"});
log("MOCK_BAMBOO backend="+r.backend+" label="+r.label);
return { ok:true };`;
  const r = run(wf);
  assert(r.code === 0, `mock bamboo provider failed: ${r.out.slice(-300)}`);
  assert(/MOCK_BAMBOO backend=mock label=mock-bamboo/.test(r.out), `mock bamboo result wrong: ${r.out.slice(-300)}`);
});

test("budget: Bamboo usage total_tokens accrues when reported", () => {
  const wf = `export const meta={name:"bu"};
await agent("x",{runtime:"bamboo",provider:"deepseek",label:"usage-bamboo",id:"usage-bamboo"});
log("BUDGET spent="+budget.spent()+" approx="+Boolean(budget.approx));
return { ok:true };`;
  const r = run(wf, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: "bamboo_usage" }, input: { budget: { total: 1000 } } });
  assert(r.code === 0, `bamboo usage run failed: ${r.out.slice(-300)}`);
  assert(/BUDGET spent=123/.test(r.out), `bamboo usage not accrued: ${r.out.slice(-500)}`);
  assert(r.state.budget?.spent === 123, `state budget spent wrong: ${JSON.stringify(r.state.budget)}`);
  assert(r.state.budget?.approx !== true, `usage-backed bamboo node should not mark approx: ${JSON.stringify(r.state.budget)}`);
});

test("doctor: Bamboo is reported but missing api_key does not fail ODW top-level health", () => {
  const root = mkdtempSync(join(tmpRoot, "doctor-"));
  const init = runOdw(["init", "--path", root], { pandacodeBin: fakePanda });
  assert(init.code === 0, `doctor fixture init failed: ${init.out.slice(-300)}`);
  const r = runOdw(["doctor", "--path", root, "--pandacode-bin", fakePanda], {
    pandacodeBin: fakePanda,
    env: { DEEPSEEK_API_KEY: "set-for-selftest" }
  });
  assert(r.code === 0, `doctor should stay ok with optional Bamboo missing key: ${r.out.slice(-500)}`);
  for (const label of ["pandacode", "codex", "claude", "bamboo", "deepseek", "kimi", "qwen", "zhipu", "minimax", "xiaomi", "stepfun"]) {
    assert(r.out.includes(label), `doctor human summary missing ${label}: ${r.out.slice(-800)}`);
  }
  assert(/deepseek ✅/.test(r.out), `doctor human summary should show configured deepseek: ${r.out.slice(-800)}`);
  assert(/zhipu ❌ set ZHIPU_API_KEY/.test(r.out), `doctor human summary should show missing zhipu key: ${r.out.slice(-800)}`);

  const jsonRun = runOdw(["doctor", "--path", root, "--pandacode-bin", fakePanda, "--json"], {
    pandacodeBin: fakePanda,
    env: { DEEPSEEK_API_KEY: "set-for-selftest" }
  });
  assert(jsonRun.code === 0, `doctor --json should stay ok with optional Bamboo missing key: ${jsonRun.out.slice(-500)}`);
  const report = JSON.parse(jsonRun.out);
  assert(report.ok === true, `doctor top-level ok should be true: ${r.out.slice(-500)}`);
  assert(report.runtimes?.bamboo?.runtime === "bamboo", `doctor missing bamboo report: ${r.out.slice(-500)}`);
  assert(report.runtimes.bamboo.missing?.includes("api_key"), `doctor bamboo missing api_key not surfaced: ${r.out.slice(-500)}`);
  assert(report.bamboo_keys?.deepseek?.ok === true, `doctor --json missing deepseek key state: ${jsonRun.out.slice(-500)}`);
  assert(report.bamboo_keys?.zhipu?.ok === false, `doctor --json missing zhipu unset state: ${jsonRun.out.slice(-500)}`);
});

test("pandacode: bamboo summary.summary becomes the node's final text", () => {
  const r = run(`export const meta={name:"bs"};
const t = await agent("hi", { runtime:"bamboo", provider:"deepseek", label:"b" });
log("BAMBOO text="+JSON.stringify(typeof t === "string" ? t : (t && t.text)));
return {ok:true};`, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: "bamboo_reply" } });
  assert(r.code === 0, `run failed: ${r.out.slice(-200)}`);
  assert(/BAMBOO text="BAMBOO-REPLY-TEXT"/.test(r.out), `bamboo reply (summary.summary) not extracted: ${r.out.slice(-300)}`);
});

test("pandacode: worktree node cleaned up when the executor fails (no orphan)", () => {
  const wf = `export const meta={name:"fw"};
const r = await agent("x",{runtime:"codex",isolation:"worktree",label:"n"});
return { ok: r?.ok !== false };`;
  const r = run(wf, { backend: "pandacode", pandacodeBin: fakePanda, env: { FAKE_PANDA: "exit1_nook" } });
  assert(r.code !== 0, `worktree-failure run must fail: ${r.out.slice(-200)}`);
  assert(odwWorktreeLeftovers().length === 0, `orphan worktree after failure:\n${odwWorktreeLeftovers().join("\n")}`);
});

// 11. .d.ts contract matches the real sandbox globals (no drift) -------------
test("contract: workflow-api.d.ts globals exactly match the runtime sandbox", () => {
  const dts = readFileSync(join(REPO, "src/pack/templates/odw/framework/workflow-api.d.ts"), "utf8");
  const runner = readFileSync(join(REPO, "src/pack/templates/runtime/odw-js-runner.mjs"), "utf8");
  const dtsNames = new Set([...dts.matchAll(/export declare (?:const|function) (\w+)/g)].map((m) => m[1]));
  const block = runner.match(/function workflowSandboxGlobals\([^)]*\)\s*\{[\s\S]*?return \{([\s\S]*?)\};/);
  assert(block, "could not locate workflowSandboxGlobals return object");
  const JS_BUILTINS = new Set(["console", "setTimeout", "clearTimeout", "Date", "Math"]);
  const sandbox = new Set([...block[1].matchAll(/^\s*(\w+)\s*[:,]/gm)].map((m) => m[1]).filter((n) => !JS_BUILTINS.has(n)));
  // Stale declarations an author would trip on (removed in convergence):
  assert(!dtsNames.has("codex"), ".d.ts still declares removed `codex` namespace");
  assert(!dtsNames.has("cwd"), ".d.ts declares `cwd`, which is not a sandbox global");
  const missingInDts = [...sandbox].filter((n) => !dtsNames.has(n));
  const extraInDts = [...dtsNames].filter((n) => !sandbox.has(n));
  assert(missingInDts.length === 0, `.d.ts is missing sandbox globals: ${missingInDts.join(", ")}`);
  assert(extraInDts.length === 0, `.d.ts declares non-existent globals: ${extraInDts.join(", ")}`);
});

// 12. Resume / state robustness ---------------------------------------------
test("resume: corrupt state.json fails loudly instead of silently restarting", () => {
  const first = run(`export const meta={name:"rc"};
await agent("a",{id:"a",mockTokens:100});
return {ok:true};`);
  assert(first.code === 0 && first.runId, `first run failed: ${first.out.slice(-200)}`);
  writeFileSync(join(first.runDir, "state.json"), "{ this is : not json");
  const resumed = run(null, { resume: first.runId });
  assert(resumed.code !== 0, `resume of corrupt state should fail, got code ${resumed.code}: ${resumed.out.slice(-200)}`);
  assert(/corrupt/i.test(resumed.out), `expected a 'corrupt' error: ${resumed.out.slice(-300)}`);
});

test("resume: editing a node prompt (same id) re-runs it via fingerprint, sibling still skips", () => {
  const sp = writeScript(`export const meta={name:"fp"};
await agent("PROMPT-A",{id:"node1",label:"node1"});
await agent("STABLE",{id:"node2",label:"node2"});
return {ok:true};`);
  const first = run(null, { scriptPath: sp });
  assert(first.code === 0 && first.runId, `first failed: ${first.out.slice(-200)}`);
  // Edit only node1's prompt, then resume the same run.
  writeFileSync(sp, `export const meta={name:"fp"};
await agent("PROMPT-B",{id:"node1",label:"node1"});
await agent("STABLE",{id:"node2",label:"node2"});
return {ok:true};`);
  const resumed = run(null, { resume: first.runId });
  assert(resumed.code === 0, `resume failed: ${resumed.out.slice(-300)}`);
  const started1 = ev(resumed.events, "agent_start").some((e) => e.key === "node1");
  const skipped1 = ev(resumed.events, "agent_skip").some((e) => e.key === "node1");
  const skipped2 = ev(resumed.events, "agent_skip").some((e) => e.key === "node2");
  assert(started1 && !skipped1, `edited node1 must re-run not skip (started=${started1} skipped=${skipped1})`);
  assert(skipped2, `unchanged node2 must still skip on resume`);
});

// 13. Observability: failures are persisted ---------------------------------
test("observability: a workflow throw persists error + failedAt to state.json", () => {
  const r = run(`export const meta={name:"obs"};
phase("P","");
throw new Error("boom-observable");`);
  assert(r.code !== 0, "throwing workflow should exit non-zero");
  assert(r.state.error && /boom-observable/.test(r.state.error.message || ""), `error not persisted: ${JSON.stringify(r.state.error)}`);
  assert(Boolean(r.state.failedAt), "failedAt not persisted");
});

// 14. report: HTML execution graph from a run --------------------------------
test("report: odw report --run renders an HTML execution graph", () => {
  const r = run(`export const meta={name:"rp"};
phase("P","");
await parallel([ () => agent("alpha task",{label:"a",runtime:"codex",model:"gpt-5-codex"}), () => agent("beta task",{label:"b",runtime:"claude"}) ]);
return {ok:true};`);
  assert(r.code === 0 && r.runId, `run failed: ${r.out.slice(-200)}`);
  const rep = spawnSync(ODW, ["report", "--path", REPO, "--run", r.runId], { cwd: REPO, encoding: "utf8" });
  assert((rep.status ?? 1) === 0, `report failed: ${((rep.stdout || "") + (rep.stderr || "")).slice(-300)}`);
  const htmlPath = (rep.stdout || "").trim().split(/\r?\n/).pop();
  const html = readFileSync(htmlPath, "utf8");
  assert(/flowchart TB/.test(html), "no mermaid graph in report");
  assert(/"runtime":"codex"/.test(html) && /"runtime":"claude"/.test(html), "node runtimes missing in report");
  assert(/"model":"gpt-5-codex"/.test(html), "node model missing in report");
  assert(/config \(from code\)/.test(html) && /"prompt":"alpha task"/.test(html), "report missing config/prompt UI parsed from code");
});

test("observability: a model the script left implicit is backfilled from the executor", () => {
  const r = run(`export const meta={name:"rm2"};
await agent("x",{label:"x", mockResolvedModel:"deepseek-v4-pro"});
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-200)}`);
  const node = Object.values(r.state.agents || {})[0] || {};
  assert(node.model === "deepseek-v4-pro", `model not backfilled in state: ${node.model}`);
  assert(
    r.events.some((e) => e.type === "agent_done" && e.model === "deepseek-v4-pro"),
    "agent_done event missing the resolved model"
  );
});

test("observability: claude completion marker never leaks into a node's returned text", () => {
  const r = run(`export const meta={name:"mk"};
const t = await agent("x",{label:"x", mockAgentText:"All good here\\nPANDACODE_DONE_1780000000000_4242"});
log("GOT["+t+"]");
return {ok:true};`);
  assert(r.code === 0, `run failed: ${r.out.slice(-200)}`);
  assert(/GOT\[All good here\]/.test(r.out), `marker not stripped: ${(r.out.match(/GOT\[[^\]]*\]/) || [])[0]}`);
  assert(!/PANDACODE_DONE/.test(r.out), "PANDACODE_DONE marker leaked into the node result");
});

test("budget: every dispatched attempt accrues tokens, even failed/retried ones", () => {
  const r = run(`export const meta={name:"bgt"};
const res = await agent("x",{label:"x", mockFail:true, mockRetryable:true, maxAttempts:3, mockTokens:100});
log("SPENT="+budget.spent()+" failed="+(res?.ok===false));
return {ok:true};`, { input: { budget: { total: 1000000 } } });
  assert(r.code === 0, `run failed: ${r.out.slice(-200)}`);
  // 3 attempts × 100 tokens accrue even though the node ultimately fails.
  assert(/SPENT=300 failed=true/.test(r.out), `budget under/over-counted retries: ${(r.out.match(/SPENT=\S+ failed=\S+/) || [])[0]}`);
});

// ---- run all --------------------------------------------------------------
let pass = 0;
const failures = [];
for (const c of cases) {
  try {
    c.fn();
    pass += 1;
    console.log(`ok   - ${c.name}`);
  } catch (e) {
    failures.push({ name: c.name, error: String(e?.message ?? e) });
    console.log(`FAIL - ${c.name}\n       ${String(e?.message ?? e)}`);
  }
}
rmSync(tmpRoot, { recursive: true, force: true });

console.log(`\n${pass}/${cases.length} passed` + (failures.length ? `, ${failures.length} failed` : ""));
if (failures.length) {
  process.exit(1);
}
