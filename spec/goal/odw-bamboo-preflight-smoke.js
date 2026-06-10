export default async function bambooPreflightSmoke() {
  phase("Bamboo Preflight Smoke", "Verify missing-key Bamboo nodes are blocked before executor dispatch.");
  const result = await agent("Return BAMBOO_OK if this executor actually runs.", {
    id: "bamboo-preflight-qwen",
    label: "bamboo-preflight-qwen",
    runtime: "bamboo",
    provider: "qwen",
    model: "qwen3.6-flash",
    timeout: 30
  });

  return {
    ok: result?.ok === false && result?.state === "blocked" && result?.error?.category === "bamboo_missing_api_key",
    result
  };
}
