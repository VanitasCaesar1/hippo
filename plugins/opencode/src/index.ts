/**
 * opencode-plugin-hippo: hippo for OpenCode. A top-level OpenCode session in
 * hippo's workspace gets hippo's start block in its system prompt, and the
 * model gets two read-only tools, `hippo_ask` and `hippo_day`. The plugin
 * only runs the `hippo` binary; it reads nothing of hippo's itself and sends
 * nothing anywhere. Design: docs/designs/opencode-plugin.md in the hippo
 * repository.
 */

import { readConfig } from './config.ts'
import type { Context, Plugin } from './opencode.ts'
import { addStartBlock, type Warn } from './start-block.ts'
import { hippoTools } from './tools.ts'

/** Set up the plugin in `ctx`; `warn` is where problems are reported. */
export async function setup(ctx: Context, warn: Warn = message => console.warn(message)): Promise<void> {
  const { config, issues } = readConfig(ctx.options)
  for (const issue of issues) warn(`hippo: option ${issue}`)
  await addStartBlock(ctx, config, warn)
  if (config.tools) {
    const tools = hippoTools(config)
    await ctx.tool.transform(editor => {
      for (const tool of tools) editor.add(tool)
    })
  }
}

const plugin: Plugin = { id: 'hippo', setup: ctx => setup(ctx) }

export default plugin
