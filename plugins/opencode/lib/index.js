/**
 * opencode-plugin-hippo: hippo for OpenCode. A top-level OpenCode session in
 * hippo's workspace gets hippo's start block in its system prompt, and the
 * model gets two read-only tools, `hippo_ask` and `hippo_day`. The plugin
 * only runs the `hippo` binary; it reads nothing of hippo's itself and sends
 * nothing anywhere. Design: docs/designs/opencode-plugin.md in the hippo
 * repository.
 */
import { readConfig } from "./config.js";
import { addStartBlock } from "./start-block.js";
import { hippoTools } from "./tools.js";
/** Set up the plugin in `ctx`; `warn` is where problems are reported. */
export async function setup(ctx, warn = message => console.warn(message)) {
    const { config, issues } = readConfig(ctx.options);
    for (const issue of issues)
        warn(`hippo: option ${issue}`);
    await addStartBlock(ctx, config, warn);
    if (config.tools) {
        const tools = hippoTools(config);
        await ctx.tool.transform(editor => {
            for (const tool of tools)
                editor.add(tool);
        });
    }
}
const plugin = { id: 'hippo', setup: ctx => setup(ctx) };
export default plugin;
