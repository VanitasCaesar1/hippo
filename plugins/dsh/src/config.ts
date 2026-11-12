/**
 * The plugin's configuration and its validator. Cordis takes any Standard
 * Schema (`~standard.validate`) as a plugin's `Config`, so this one is
 * written by hand instead of pulling in a schema library.
 */

export interface Config {
  /** The hippo binary: a name looked up on PATH (then ~/.local/bin, ~/.cargo/bin), or a path. */
  command: string
  /** How long one hippo command may run, in milliseconds. */
  timeoutMs: number
  /** Register the hippo_ask and hippo_day tools; their results reach the session's model. */
  tools: boolean
}

export const defaults: Config = { command: 'hippo', timeoutMs: 10_000, tools: true }

interface Issue {
  message: string
  path?: string[]
}

type Result = { value: Config; issues?: undefined } | { issues: Issue[] }

/** Fill defaults and check each field; an unknown field is an error. */
export function validate(input: unknown): Result {
  if (input !== undefined && input !== null && (typeof input !== 'object' || Array.isArray(input))) {
    return { issues: [{ message: 'dsh-plugin-hippo: config must be an object' }] }
  }
  const issues: Issue[] = []
  const value: Config = { ...defaults }
  for (const [key, raw] of Object.entries(input ?? {})) {
    if (raw === undefined) continue
    const issue = (message: string) => issues.push({ message: `dsh-plugin-hippo: ${key} ${message}`, path: [key] })
    if (key === 'command') {
      if (typeof raw === 'string' && raw.trim() !== '') value.command = raw
      else issue('must be a non-empty string')
    } else if (key === 'timeoutMs') {
      if (typeof raw === 'number' && Number.isSafeInteger(raw) && raw > 0) value.timeoutMs = raw
      else issue('must be a positive whole number')
    } else if (key === 'tools') {
      if (typeof raw === 'boolean') value.tools = raw
      else issue('must be true or false')
    } else {
      issue('is not a setting this plugin has')
    }
  }
  return issues.length > 0 ? { issues } : { value }
}

/** The plugin's `Config` export: a Standard Schema (version 1). */
export const Config = {
  '~standard': {
    version: 1 as const,
    vendor: 'dsh-plugin-hippo',
    validate,
  },
}
