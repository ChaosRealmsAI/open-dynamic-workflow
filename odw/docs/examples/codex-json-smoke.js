export const meta = {
  name: "codex-json-smoke",
  promptSlots: ["plan_decompose"]
};

phase("Plan", "Ask PandaCode Codex for arbitrary structured JSON");
return agent(promptSlot("plan_decompose", {
  required_schema: ".odw/schemas/task-plan.schema.json"
}), {
  id: "plan-json",
  label: "pandacode codex schema",
  phase: "Plan",
  runtime: "codex",
  schema: ".odw/schemas/task-plan.schema.json",
  schemaDescription: "Final response is an arbitrary task-plan object used to verify PandaCode Codex schema-guided extraction.",
  retry: { maxAttempts: 1 }
});
