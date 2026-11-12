/**
 * dsh-plugin-hippo: hippo for DeepSeek Harness (dsh). A new dsh session in
 * hippo's workspace starts with hippo's start block, and the model gets two
 * read-only tools, `hippo_ask` and `hippo_day`. The plugin only runs the
 * `hippo` binary; it reads nothing of hippo's itself and sends nothing
 * anywhere. Design: docs/designs/dsh-plugin.md in the hippo repository.
 */

import { Config, validate } from './config.ts'
import type { Context } from './dsh.ts'
import { addStartBlock, projection } from './start-block.ts'
import { hippoTools } from './tools.ts'

export { Config }

/** The Cordis plugin name. */
export const name = 'hippo'

/** The dsh services the plugin uses. */
export const inject = ['agents', 'tools', 'sessionProjections', 'workingDirectory']

export async function apply(ctx: Context, input?: Partial<Config>): Promise<void> {
  // Cordis validates through `Config` before `apply`; validate again for a
  // caller that does not, such as a test.
  const checked = validate(input)
  if (checked.issues) throw new TypeError(checked.issues.map(issue => issue.message).join('; '))
  const config = checked.value
  if (config.tools) for (const tool of hippoTools(config)) ctx.tools.register(tool)
  const handle = ctx.sessionProjections.register(projection)
  addStartBlock(ctx, config, handle)
  await handle
}
