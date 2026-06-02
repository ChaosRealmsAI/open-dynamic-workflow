#!/usr/bin/env node
// odw execution-graph report generator.
//
//   node odw-report.mjs <runDir> <outHtmlPath> <vendorDir>
//
// Reads <runDir>/events.jsonl (+ state.json), reconstructs the execution graph,
// and writes a self-contained HTML: left = Mermaid graph (uncoloured), right =
// each node's LITERAL config parsed from the workflow code (runtime, model,
// provider, schema, isolation, ...), its prompt, and its execution result
// (status, tokens, duration). No editorialising — only what the code declares
// and what the run produced. mermaid/marked are loaded from <vendorDir>.

import { readFileSync, writeFileSync } from "node:fs";
import { relative, dirname, join } from "node:path";

const [runDir, outHtml, vendorDir] = process.argv.slice(2);
if (!runDir || !outHtml || !vendorDir) {
  console.error("usage: node odw-report.mjs <runDir> <outHtmlPath> <vendorDir>");
  process.exit(2);
}

const TEMPLATE = String.raw`<!doctype html><html lang=zh><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1"><title>__TITLE__</title>
<style>
*{box-sizing:border-box}
:root{--bg:#0b0c10;--panel:#0f1117;--line:#1d2029;--line2:#2a2e39;--ink:#edeff4;--ink2:#aeb3c1;--dim:#777d8c;--accent:#7c83ff;--ok:#35c79a;--fail:#e0574b}
html,body{margin:0;height:100%;background:var(--bg);color:var(--ink);font-family:-apple-system,"SF Pro Display","Segoe UI",system-ui,sans-serif;-webkit-font-smoothing:antialiased}
.top{display:flex;align-items:baseline;gap:14px;padding:14px 24px;border-bottom:1px solid var(--line);background:#0a0b0e}
.top .ttl{font-size:15px;font-weight:600}.top .sub{color:var(--dim);font-size:12.5px}
.wrap{display:flex;height:calc(100vh - 52px)}
.graph{flex:1.5;display:flex;align-items:center;justify-content:center;overflow:hidden;padding:28px;background:radial-gradient(800px 480px at 42% 6%,rgba(124,131,255,.04),transparent 60%)}
.graph pre.mermaid{margin:0}.graph svg{max-width:none!important;display:block}
.node{cursor:pointer}.node *{transition:filter .15s}
.node.sel>*{stroke:var(--accent)!important;stroke-width:2.4px!important;filter:brightness(1.25) drop-shadow(0 0 8px rgba(124,131,255,.4))}
.side{flex:1;max-width:560px;min-width:420px;overflow:auto;background:var(--panel);border-left:1px solid var(--line)}
.detail{padding:24px 26px 48px}
.back{display:inline-block;color:var(--dim);font-size:12.5px;cursor:pointer;margin-bottom:14px;user-select:none}.back:hover{color:var(--ink2)}
.dname{font-size:21px;font-weight:600;margin-bottom:4px}
.dkind{color:var(--dim);font-size:12px;margin-bottom:16px}
.lab{margin:20px 0 8px;font-size:10.5px;letter-spacing:1.2px;text-transform:uppercase;color:var(--dim);font-weight:600}
.kv{display:grid;grid-template-columns:max-content 1fr;gap:6px 16px;font-size:13px}
.kv .k{color:var(--dim);font-family:ui-monospace,Menlo,monospace;font-size:12px}
.kv .v{color:var(--ink);font-family:ui-monospace,Menlo,monospace;font-size:12.5px;word-break:break-word}
.kv .v.ok{color:var(--ok)}.kv .v.fail{color:var(--fail)}
.prompt{background:#0a0b0f;border:1px solid var(--line);border-radius:10px;padding:14px 15px;font-size:12.5px;line-height:1.7;color:var(--ink2);white-space:pre-wrap;word-break:break-word;font-family:ui-monospace,Menlo,monospace;max-height:46vh;overflow:auto}
.empty{color:var(--dim);font-size:13px}
</style></head><body>
<div class="top"><span class="ttl">__TITLE__</span><span class="sub">__SUBTITLE__</span></div>
<div class="wrap">
 <div class="graph"><pre class="mermaid">__GRAPH__</pre></div>
 <div class="side"><div class="detail" id="detail"></div></div>
</div>
<script src="__MERMAID__"></script>
<script src="__MARKED__"></script>
<script>
const NODES=__NODES__;
const OVERVIEW=__OVERVIEW__;
function esc(s){return String(s==null?"":s).replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;");}
function num(n){return (typeof n==="number")?n.toLocaleString("en-US"):"—";}
function dur(n){return (typeof n==="number")?(n>=1000?(n/1000).toFixed(1)+"s":n+"ms"):"—";}
// Config keys shown verbatim from the agent() call (only ones the code set).
const CFG=[["runtime","runtime"],["model","model"],["provider","provider"],["schema","schema"],["isolation","isolation"],["agentType","agentType"],["effort","effort"],["timeout","timeout"],["maxAttempts","maxAttempts"]];
function row(k,v,cls){return '<div class="k">'+k+'</div><div class="v'+(cls?' '+cls:'')+'">'+esc(v)+'</div>';}
function detail(n){
 let h='<a class="back" onclick="showOverview()">‹ overview</a>';
 h+='<div class="dname">'+esc(n.label)+'</div>';
 h+='<div class="dkind">'+(n.kind==='ai'?'agent() node':'code')+(n.stage?' · '+esc(n.stage):'')+'</div>';
 if(n.kind!=='ai'){h+='<div class="empty">Orchestration step (parallel / pipeline fan-out / join / start / end) — emitted by the workflow code, not an AI call.</div>';return h;}
 // config straight from the code
 const cfg=n.config||{};
 let rows='';
 for(const [key,src] of CFG){ if(cfg[src]!=null&&cfg[src]!=='') rows+=row(key,cfg[src]); }
 h+='<div class="lab">config (from code)</div><div class="kv">'+(rows||'<div class="k">—</div><div class="v">(defaults)</div>')+'</div>';
 // execution result
 const st=n.status; const stCls=st==='ok'?'ok':(st==='failed'?'fail':'');
 h+='<div class="lab">result</div><div class="kv">'+
   row('status',st||'—',stCls)+row('tokens',num(n.tokens))+row('duration',dur(n.durationMs))+'</div>';
 // the prompt, verbatim
 h+='<div class="lab">prompt</div>'+(n.prompt?'<div class="prompt">'+esc(n.prompt)+'</div>':'<div class="empty">(none)</div>');
 return h;
}
let _sel=null;
function svgNodeEl(id){return document.querySelector('.graph .node[id*="flowchart-'+id+'-"]');}
function selectNode(id,el){const n=NODES[id];if(!n)return;el=el||svgNodeEl(id);
 if(_sel)_sel.classList.remove('sel');if(el){el.classList.add('sel');_sel=el;}
 document.getElementById('detail').innerHTML=detail(n);}
function showOverview(){
 if(_sel){_sel.classList.remove('sel');_sel=null;}
 const o=OVERVIEW;
 let h='<div class="dname">'+esc(o.name)+'</div><div class="dkind">'+esc(o.subtitle)+'</div>';
 h+='<div class="lab">run</div><div class="kv">'+
   row('backend',o.backend)+row('status',o.status,o.failed?'fail':'ok')+
   row('agent nodes',num(o.ai))+row('total tokens',num(o.tokens)+(o.approx?' (≥)':''))+'</div>';
 h+='<div class="lab" style="margin-top:22px">tip</div><div class="empty">Click any node to see its config and prompt as written in the workflow code.</div>';
 document.getElementById('detail').innerHTML=h;}
function fitGraph(){const svg=document.querySelector('.graph svg'),g=document.querySelector('.graph');
 if(!svg||!g)return;const vb=svg.viewBox&&svg.viewBox.baseVal;if(!vb||!vb.width)return;
 const s=Math.min((g.clientWidth-56)/vb.width,(g.clientHeight-56)/vb.height);
 svg.style.maxWidth='none';svg.style.width=(vb.width*s)+'px';svg.style.height=(vb.height*s)+'px';}
async function boot(){
 if(!window.mermaid){document.getElementById('detail').textContent='mermaid not loaded';return;}
 mermaid.initialize({startOnLoad:false,theme:'dark',securityLevel:'loose',flowchart:{curve:'basis',nodeSpacing:36,rankSpacing:50,htmlLabels:true,padding:12},themeVariables:{fontFamily:'inherit',lineColor:'#3a3e4a'}});
 await mermaid.run({querySelector:'pre.mermaid'});
 fitGraph();
 document.querySelectorAll('.graph .node').forEach(el=>{const id=el.id.replace(/^.*flowchart-/,'').replace(/-\d+$/,'');el.addEventListener('click',(e)=>{e.stopPropagation();selectNode(id,el);});});
 document.querySelector('.graph').addEventListener('click',()=>showOverview());
 showOverview();
}
window.addEventListener('resize',fitGraph);
boot();
</script></body></html>`;

function readEvents(dir) {
  let text;
  try { text = readFileSync(join(dir, "events.jsonl"), "utf8"); } catch { return []; }
  return text.split(/\r?\n/).filter(Boolean).map((line) => {
    try { const e = JSON.parse(line); return e && e.raw ? e.raw : e; } catch { return null; }
  }).filter(Boolean);
}
function readState(dir) {
  try { return JSON.parse(readFileSync(join(dir, "state.json"), "utf8")); } catch { return {}; }
}

const events = readEvents(runDir);
const state = readState(runDir);
const ms = (a, b) => (a && b ? Math.max(0, new Date(b).getTime() - new Date(a).getTime()) : null);

// ---- reconstruct the graph from the event timeline -----------------------
const nodes = {};
const order = [];
let codeSeq = 0;
function addCode(id, label, term = false) { nodes[id] = { id, kind: "code", label, term }; order.push(id); return id; }
const startId = addCode("start", "start", true);
const edges = [];
const link = (a, b) => { if (a && b) edges.push([a, b]); };
const groups = [];
let tail = startId;
const startTs = {};

for (const ev of events) {
  const t = ev.type;
  if (t === "parallel_start" || t === "pipeline_start") {
    codeSeq += 1;
    const forkId = addCode(`fork${codeSeq}`, t === "pipeline_start" ? "pipeline" : "parallel");
    link(tail, forkId);
    groups.push({ forkId, children: [] });
    tail = forkId;
  } else if (t === "parallel_done" || t === "pipeline_done") {
    const g = groups.pop();
    if (g) {
      codeSeq += 1;
      const joinId = addCode(`join${codeSeq}`, "join");
      if (g.children.length === 0) link(g.forkId, joinId);
      else for (const c of g.children) link(c, joinId);
      tail = joinId;
    }
  } else if (t === "agent_start") {
    startTs[ev.key] = ev.ts;
    if (!nodes[ev.key]) {
      nodes[ev.key] = { id: ev.key, kind: "ai", label: ev.label || ev.key, stage: ev.phase || "", config: ev.config || {}, prompt: ev.promptPreview || "", status: "running", tokens: null, durationMs: null };
      order.push(ev.key);
      const g = groups[groups.length - 1];
      if (g) { link(g.forkId, ev.key); g.children.push(ev.key); }
      else { link(tail, ev.key); tail = ev.key; }
    }
  } else if (t === "agent_done") {
    const n = nodes[ev.key];
    if (n) { n.status = ev.ok === false ? "failed" : "ok"; if (typeof ev.tokens === "number") n.tokens = ev.tokens; n.durationMs = ms(startTs[ev.key], ev.ts);
      // Prefer the model the executor actually resolved over a code-declared "inherit".
      if (ev.model && (!n.config.model || n.config.model === "inherit")) n.config = { ...n.config, model: ev.model }; }
  } else if (t === "agent_skip") {
    if (!nodes[ev.key]) { nodes[ev.key] = { id: ev.key, kind: "ai", label: ev.label || ev.key, stage: ev.phase || "", config: ev.config || {}, prompt: "", status: "skip", tokens: null, durationMs: null }; order.push(ev.key); const g = groups[groups.length - 1]; if (g) { link(g.forkId, ev.key); g.children.push(ev.key); } else { link(tail, ev.key); tail = ev.key; } }
    else nodes[ev.key].status = "skip";
  }
}
const endId = addCode("end", "end", true);
link(tail, endId);

// ---- totals + overview ----------------------------------------------------
const aiNodes = order.map((id) => nodes[id]).filter((n) => n.kind === "ai");
const totalTokens = (state.budget && typeof state.budget.spent === "number") ? state.budget.spent : aiNodes.reduce((s, n) => s + (n.tokens || 0), 0);
const wfStart = events.find((e) => e.type === "workflow_start");
const wfErr = events.find((e) => e.type === "workflow_error");
const wfDone = events.find((e) => e.type === "workflow_done");
const status = wfErr ? "failed" : (wfDone ? "ok" : "running");
const backend = (wfStart && wfStart.backend) || state.backend || "?";
const name = (wfStart && wfStart.name) || (state.workflow && state.workflow.name) || "workflow";
const overview = { name, subtitle: `${backend} · ${aiNodes.length} nodes`, backend, status, failed: Boolean(wfErr) || aiNodes.some((n) => n.status === "failed"), ai: aiNodes.length, tokens: totalTokens, approx: Boolean(state.budget && state.budget.approx) };

// ---- mermaid (uncoloured) -------------------------------------------------
const safe = (id) => "n_" + String(id).replace(/[^a-zA-Z0-9_]/g, "_");
function mermaid() {
  const L = ["flowchart TB"];
  for (const id of order) {
    const n = nodes[id];
    const lbl = String(n.label).replace(/"/g, "”").slice(0, 24);
    const shape = n.term ? `(["${lbl}"])` : (n.kind === "code" ? `{"${lbl}"}` : `("${lbl}")`);
    const cls = n.kind === "ai" ? (n.status === "failed" ? "fail" : "node") : "code";
    L.push(`  ${safe(id)}${shape}:::${cls}`);
  }
  for (const [a, b] of edges) L.push(`  ${safe(a)} --> ${safe(b)}`);
  L.push("  classDef node fill:#161922,stroke:#586074,color:#e3e6ef,stroke-width:1.3px;");
  L.push("  classDef fail fill:#241317,stroke:#e0574b,color:#f3d2cf,stroke-width:1.3px;");
  L.push("  classDef code fill:#101218,stroke:#363a45,color:#9aa0b0,stroke-width:1.1px;");
  return L.join("\n");
}

const njson = {};
for (const id of order) {
  const n = nodes[id];
  njson[safe(id)] = { kind: n.kind, label: n.label, stage: n.stage, config: n.config || {}, prompt: n.prompt, status: n.status, tokens: n.tokens, durationMs: n.durationMs };
}

const vendorRel = (file) => {
  const r = relative(dirname(outHtml), join(vendorDir, file)).split("\\").join("/");
  return r.startsWith(".") ? r : "./" + r;
};
const sub = (s) => () => s;
const escAttr = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const html = TEMPLATE
  .replace(/__TITLE__/g, sub(escAttr(name)))
  .replace(/__SUBTITLE__/g, sub(escAttr(`${backend} · ${status} · ${aiNodes.length} nodes · ${totalTokens.toLocaleString("en-US")} tokens`)))
  .replace("__GRAPH__", sub(mermaid()))
  .replace("__NODES__", sub(JSON.stringify(njson)))
  .replace("__OVERVIEW__", sub(JSON.stringify(overview)))
  .replace("__MERMAID__", sub(vendorRel("mermaid.min.js")))
  .replace("__MARKED__", sub(vendorRel("marked.min.js")));

writeFileSync(outHtml, html);
console.log(outHtml);
