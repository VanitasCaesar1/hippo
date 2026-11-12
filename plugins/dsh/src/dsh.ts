/**
 * The parts of DeepSeek Harness's plugin API this plugin uses, declared by
 * shape. The plugin imports nothing from dsh at run time; these types say
 * what it expects `ctx` to hold. Read from dsh's source at commit e8f7b93
 * (0.2.2-rc.2): `@deepseek-ai/cordis`, `dsh-agent`, `dsh-tools`,
 * `dsh-session`, `dsh-session-projection` and `dsh-working-directory`.
 */

export interface TextBlock {
  readonly type: 'text'
  readonly text: string
}

/** A user-role message, as dsh's `createUserMessage` makes one. */
export interface UserMessage {
  readonly id: string
  readonly role: 'user'
  readonly content: readonly TextBlock[]
  readonly source: { readonly kind: string; readonly [field: string]: unknown }
}

export interface SessionHeader {
  readonly id: string
  readonly cwd?: string
  readonly origin?: 'subagent'
}

export interface Session {
  readonly header: SessionHeader
}

export interface Agent {
  readonly session: Session
}

/** One committed session event; only `user/message` is read here. */
export interface SessionEvent {
  readonly type: string
  readonly seq?: number
  readonly data?: unknown
}

export type PreStepDecision =
  | { kind: 'reject' }
  | { kind: 'enter'; messages: UserMessage[]; startsRequestSeries?: true }

export interface PreStepPayload {
  readonly agent: Agent
  readonly messages: UserMessage[]
  readonly turn: number
  readonly step: number
  readonly signal: AbortSignal
}

export interface ProjectionDefinition<S> {
  key: string
  stateVersion: number
  /** dsh validates a cached state with this before it seeds a fold. */
  stateSchema: { parse(value: unknown): S }
  init(): S
  apply(state: S, event: SessionEvent): S
}

export interface ProjectionHandle<S> {
  stateOf(session: Session): S
}

/** A tool as `ctx.tools.register` takes it, without `defineTool`. */
export interface ToolDefinition {
  readonly name: string
  readonly description: string
  /** JSON Schema for the arguments; the tool checks them itself. */
  readonly parameters: Record<string, unknown>
  readonly output: {
    readonly schema: Record<string, unknown>
    render(args: unknown, value: unknown): TextBlock[]
  }
  readonly timeoutMs?: number
  execute(args: unknown, exec: { readonly signal: AbortSignal; readonly agent?: Agent }): Promise<unknown>
}

export interface Logger {
  warn(message: string): void
}

export interface Context {
  readonly logger: Logger
  on(
    event: 'agent/pre-step',
    listener: (payload: PreStepPayload, next: () => Promise<PreStepDecision>) => Promise<PreStepDecision>,
    options?: { prepend?: boolean },
  ): unknown
  readonly tools: { register(definition: ToolDefinition): unknown }
  readonly sessionProjections: {
    register<S>(definition: ProjectionDefinition<S>): Promise<ProjectionHandle<S>>
  }
  readonly workingDirectory: { get(session: Session): string }
}
