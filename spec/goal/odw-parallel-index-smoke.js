export default async function parallelIndexSmoke() {
  phase("Parallel Index Smoke", "Verify concurrent agent bookkeeping keeps unique call indexes.");
  const labels = ["alpha", "beta", "gamma", "delta", "epsilon"];
  const results = await parallel(
    labels.map((label) => () => agent(`Return ${label}.`, {
      id: `parallel-index-${label}`,
      label: `parallel-index-${label}`,
      runtime: "codex"
    })),
    { label: "parallel-index-smoke", maxConcurrency: 5 }
  );

  return {
    ok: results.length === labels.length,
    labels,
    results
  };
}
