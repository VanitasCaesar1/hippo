/**
 * The shapes of OpenCode 2's plugin context this plugin uses, written out
 * here instead of imported from `@opencode/plugin`, so the plugin has no
 * dependencies (OpenCode forbids a plugin from bundling its own copy of
 * that package anyway). Read from OpenCode's source at tag v2.2.7:
 * packages/plugin/src/promise/{plugin,session,tool}.ts.
 */

/** A part of the system prompt of one model request. */
export interface SystemPart {
  type: 'text'
  text: string
}

/** The `context` hook's event: one model request of a session's agent loop. */
export interface SessionContext {
  readonly sessionID: string
  readonly agent?: string
  system: SystemPart[]
}

/** What `ctx.session.get` returns, as far as the plugin reads it. */
export interface SessionInfo {
  id: string
  /** Set for a sub-agent's session. */
  parentID?: string
  location?: { directory?: string }
}

/** A registered hook; `dispose` removes it. */
export interface Registration {
  dispose(): Promise<void> | void
}

/** What a tool's `execute` is given besides its input. */
export interface ToolContext {
  readonly sessionID: string
  readonly signal: AbortSignal
}

/** What a tool's `execute` returns. No `output` key without an output schema. */
export interface ToolResult {
  content: string
}

/** A tool, as `editor.add` takes it. `input` is a plain JSON Schema. */
export interface ToolInfo {
  name: string
  description: string
  input: Record<string, unknown>
  options?: { codemode: boolean }
  execute(input: unknown, context: ToolContext): Promise<ToolResult>
}

export interface ToolEditor {
  add(tool: ToolInfo): void
}

/** The plugin context, as far as the plugin uses it. */
export interface Context {
  readonly location: { directory: string }
  readonly options: Readonly<Record<string, unknown>>
  readonly session: {
    hook(name: 'context', callback: (event: SessionContext) => void | Promise<void>): Promise<Registration>
    get(input: { sessionID: string }): Promise<SessionInfo>
  }
  readonly tool: {
    /** The callback must be synchronous and free of side effects: OpenCode replays it. */
    transform(callback: (editor: ToolEditor) => void): Promise<unknown>
  }
}

/** A plugin, as OpenCode 2 loads one: the module's default export. */
export interface Plugin {
  readonly id: string
  setup(ctx: Context): Promise<void>
}
