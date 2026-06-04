#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { join } from "node:path";

const runDir = process.argv[2];
if (!runDir) {
  console.error("usage: node spec/goal/analyze-odw-dogfood-quality.mjs <odw-run-dir>");
  process.exit(2);
}

const state = JSON.parse(readFileSync(join(runDir, "state.json"), "utf8"));
const entries = [
  ...Object.values(state.failedAgents || {}),
  ...Object.values(state.agents || {})
].sort((a, b) => (a.index || 0) - (b.index || 0) || String(a.key).localeCompare(String(b.key)));

function textOf(entry) {
  return typeof entry.result === "string" ? entry.result : JSON.stringify(entry.result || {});
}

function scoreEntry(entry) {
  if (entry.ok === false) {
    return { score: 0, level: "blocked", evidence: 0, verification: 0, novelty: 0, correction: 0 };
  }
  const text = textOf(entry);
  const hasFileEvidence = /(?:src|test)\/|README|package\.json|\.js|\.json/.test(text);
  const evidence = (hasFileEvidence ? 2 : 0) + (/\bEvidence\b|:\d+|file|path|command|validator/i.test(text) ? 1 : 0);
  const verification = /npm test|passes|passed|20-process|persisted|verify|overstated|missing evidence/i.test(text) ? 2 : 0;
  const novelty = /lost updates|security|concurrency|corruption|schema|migration|release|observability|error|path|risk|gap|preflight/i.test(text) ? 2 : 1;
  const correction = /overstated|missing evidence|supported|temper|verify|SYNTHESIS|EXIT_REVIEW/i.test(text) ? 2 : 0;
  const score = Math.min(10, evidence + verification + novelty + correction + (text.length > 250 ? 1 : 0));
  return {
    score,
    level: score >= 8 ? "high" : score >= 5 ? "medium" : "low",
    evidence,
    verification,
    novelty,
    correction
  };
}

function preview(entry) {
  return textOf(entry)
    .replace(/\s+/g, " ")
    .replace(/\|/g, "\\|")
    .slice(0, 130);
}

const scored = entries.map((entry) => ({ entry, quality: scoreEntry(entry) }));
const successful = scored.filter((row) => row.entry.ok !== false);
const totalScore = successful.reduce((sum, row) => sum + row.quality.score, 0);
const duplicateIndexes = [...scored.reduce((map, row) => {
  const index = row.entry.index || 0;
  if (!map.has(index)) {
    map.set(index, []);
  }
  map.get(index).push(row.entry.key);
  return map;
}, new Map()).entries()].filter(([, keys]) => keys.length > 1);

console.log(`# ODW Dogfood Quality Analysis`);
console.log();
console.log(`Run: \`${runDir}\``);
console.log(`Nodes: ${entries.length}; successful: ${successful.length}; blocked/failed: ${entries.length - successful.length}; average successful score: ${(totalScore / Math.max(1, successful.length)).toFixed(2)}/10.`);
console.log(`Duplicate state indexes: ${duplicateIndexes.length ? duplicateIndexes.map(([index, keys]) => `${index}=${keys.join(",")}`).join("; ") : "none"}.`);
console.log();
console.log("| # | node | phase | status | tokens | score | quality | note |");
console.log("|---:|---|---|---|---:|---:|---|---|");
for (const { entry, quality } of scored) {
  console.log(`| ${entry.index ?? ""} | ${entry.key} | ${entry.phase || ""} | ${entry.ok === false ? "blocked" : "ok"} | ${entry.tokens || 0} | ${quality.score} | ${quality.level} | ${preview(entry)} |`);
}
