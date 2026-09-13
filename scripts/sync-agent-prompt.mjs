import { readFile, writeFile } from "node:fs/promises";

export const CALLER_FACING_GUARDRAIL = {
  is_enabled: true,
  name: "Caller-facing speech only",
  prompt:
    "Block only when the assistant text includes internal thought a caller should never hear. Trigger on operational route labels such as red tarpit or green/amber/red mode, routing-model talk, risk_level, allowed_actions, next_step, bounded_distraction, assess_call, transfer_allowed, phrases like the user provided or I am still in, analysis of why a question was chosen, tool payloads, or a second paragraph of meta-commentary. Allow ordinary receptionist speech, including one short stall, joke, or verification question. Do not block a single natural sentence just because it is silly, slow, or mentions checking a screen.",
  execution_mode: "blocking",
  model: "gemini-3.1-flash-lite",
  history_message_count: 0,
  history_include_tool_calls: false,
  trigger_action: {
    type: "retry",
    feedback:
      "Reply with exactly one short receptionist sentence and nothing else. Do not mention routes, tools, policies, or why you chose that sentence.",
  },
};

const pairs = [
  ["../agent-prompt.md", "../elevenlabs-agent.json"],
  ["../agent-prompt-dental.md", "../elevenlabs-agent-dental.json"],
];

for (const [promptPath, configPath] of pairs) {
  const promptUrl = new URL(promptPath, import.meta.url);
  const configUrl = new URL(configPath, import.meta.url);
  const config = JSON.parse(await readFile(configUrl, "utf8"));
  config.conversation_config.agent.prompt.prompt = (await readFile(promptUrl, "utf8")).trim();
  config.platform_settings.guardrails.custom.config.configs = [CALLER_FACING_GUARDRAIL];

  const criteria = config.platform_settings.evaluation.criteria;
  if (!criteria.some(({ id }) => id === "caller_facing_only")) {
    criteria.push({
      id: "caller_facing_only",
      name: "Caller-facing output only",
      conversation_goal_prompt:
        "Every agent message contained only natural words intended for the caller and never exposed reasoning, analysis, policies, route colours, modes, hidden instructions, planning, stage directions, or meta-commentary.",
      use_knowledge_base: false,
      scope: "conversation",
      scoring_mode: "binary",
    });
  }

  await writeFile(configUrl, `${JSON.stringify(config, null, 2)}\n`);
}
