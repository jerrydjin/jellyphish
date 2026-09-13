import { readFile, writeFile } from "node:fs/promises";

const pairs = [
  ["../agent-prompt.md", "../elevenlabs-agent.json"],
  ["../agent-prompt-dental.md", "../elevenlabs-agent-dental.json"],
];

for (const [promptPath, configPath] of pairs) {
  const promptUrl = new URL(promptPath, import.meta.url);
  const configUrl = new URL(configPath, import.meta.url);
  const config = JSON.parse(await readFile(configUrl, "utf8"));
  config.conversation_config.agent.prompt.prompt = (await readFile(promptUrl, "utf8")).trim();

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
