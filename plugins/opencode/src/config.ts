/**
 * The plugin's options, from its entry in `opencode.json`:
 * `{ "package": "…", "options": { "tools": false } }`.
 */

export interface Config {
  /** The hippo binary: a name looked up on PATH (then ~/.local/bin, ~/.cargo/bin), or a path. */
  command: string
  /** How long one hippo command may run, in milliseconds. */
  timeoutMs: number
  /** Add the hippo_ask and hippo_day tools; their results reach the session's model. */
  tools: boolean
}

export const defaults: Config = { command: 'hippo', timeoutMs: 10_000, tools: true }

/**
 * Fill defaults and check each option. A wrong or unknown option is
 * reported and its default used, so a typo never stops OpenCode.
 */
export function readConfig(input: unknown): { config: Config; issues: string[] } {
  const config: Config = { ...defaults }
  const issues: string[] = []
  if (input === undefined || input === null) return { config, issues }
  if (typeof input !== 'object' || Array.isArray(input)) {
    return { config, issues: ['options must be an object; using the defaults'] }
  }
  for (const [key, raw] of Object.entries(input)) {
    if (raw === undefined) continue
    const issue = (message: string) => issues.push(`${key} ${message}`)
    if (key === 'command') {
      if (typeof raw === 'string' && raw.trim() !== '') config.command = raw
      else issue('must be a non-empty string; using hippo')
    } else if (key === 'timeoutMs') {
      if (typeof raw === 'number' && Number.isSafeInteger(raw) && raw > 0) config.timeoutMs = raw
      else issue(`must be a positive whole number; using ${defaults.timeoutMs}`)
    } else if (key === 'tools') {
      if (typeof raw === 'boolean') config.tools = raw
      else issue('must be true or false; using true')
    } else {
      issue('is not an option this plugin has')
    }
  }
  return { config, issues }
}
