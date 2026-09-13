import { readFile, writeFile } from "node:fs/promises";

const configPath = new URL("../elevenlabs-agent.json", import.meta.url);
const promptPath = new URL("../agent-prompt.md", import.meta.url);

const config = JSON.parse(await readFile(configPath, "utf8"));
config.conversation_config.agent.prompt.prompt = (await readFile(promptPath, "utf8")).trim();

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

await writeFile(configPath, `${JSON.stringify(config, null, 2)}\n`);
