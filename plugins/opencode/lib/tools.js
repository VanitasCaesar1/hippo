/**
 * The two tools: `hippo_ask` (has an agent already solved this?) and
 * `hippo_day` (what did the agents do on a day?). Both run read-only hippo
 * commands, in the home folder, and return text.
 */
import { homedir } from 'node:os';
import { runHippo } from "./hippo.js";
const MAX_QUESTION = 2000;
/** A tool argument the model got wrong; OpenCode shows the message as the tool's error. */
class ToolArgsError extends Error {
    constructor(message) {
        super(message);
        this.name = 'ToolArgsError';
    }
}
/** `+HH:MM` or `-HH:MM` for a JavaScript time-zone offset (minutes behind UTC). */
export function utcOffset(timezoneOffset) {
    const minutes = -timezoneOffset;
    const sign = minutes < 0 ? '-' : '+';
    const abs = Math.abs(minutes);
    const pad = (n) => String(n).padStart(2, '0');
    return `${sign}${pad(Math.floor(abs / 60))}:${pad(abs % 60)}`;
}
/** The machine's UTC offset on `date` (`YYYY-MM-DD`, at noon), or now. */
export function offsetFor(date) {
    if (date === undefined)
        return utcOffset(new Date().getTimezoneOffset());
    const [y, m, d] = date.split('-').map(Number);
    return utcOffset(new Date(y, m - 1, d, 12).getTimezoneOffset());
}
function argsObject(args) {
    if (args === null || typeof args !== 'object' || Array.isArray(args))
        throw new ToolArgsError('arguments must be an object');
    return args;
}
function noOthers(rest) {
    const [other] = Object.keys(rest);
    if (other !== undefined)
        throw new ToolArgsError(`unknown argument ${other}`);
}
export function askArgs(args) {
    const { question, ...rest } = argsObject(args);
    noOthers(rest);
    if (typeof question !== 'string' || question.trim() === '')
        throw new ToolArgsError('question must be a non-empty string');
    if (question.length > MAX_QUESTION)
        throw new ToolArgsError(`question must be at most ${MAX_QUESTION} characters`);
    // `--` so a question that starts with `-` is never read as a flag.
    return ['ask', '--sources-only', '--json', '--', question.trim()];
}
export function dayArgs(args, offset = offsetFor) {
    const { date, ...rest } = argsObject(args ?? {});
    noOthers(rest);
    if (date === undefined)
        return ['show', 'day', '--offset', offset()];
    if (typeof date !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(date) || Number.isNaN(Date.parse(`${date}T00:00:00Z`))) {
        throw new ToolArgsError('date must be a day as YYYY-MM-DD');
    }
    return ['show', 'day', date, '--offset', offset(date)];
}
const oneLine = (text) => String(text ?? '').split(/\s+/).filter(Boolean).join(' ');
/**
 * `hippo ask --json` as text, the way `hippo ask` prints it plus each
 * source's session id. Output that is not the JSON expected comes back as
 * it is.
 */
export function renderAsk(stdout) {
    let asked;
    try {
        asked = JSON.parse(stdout);
    }
    catch {
        return stdout;
    }
    if (asked === null || typeof asked !== 'object' || !Array.isArray(asked.sources))
        return stdout;
    const terms = Array.isArray(asked.terms) ? asked.terms.join(' ') : '';
    if (terms === '')
        return 'nothing to search for in that question\n';
    const lines = [`searched ${asked.searched?.sessions ?? 0} sessions and ${asked.searched?.notes ?? 0} notes for: ${terms}`];
    if (asked.sources.length === 0)
        lines.push('no session or note matched');
    for (const s of asked.sources) {
        const facts = [s.kind, s.tool ?? s.profile, s.project, s.time?.slice(0, 16).replace('T', ' ')].filter(Boolean);
        lines.push(`[${s.n}] ${oneLine(s.title)}  (${facts.join(' · ')})`);
        if (s.why)
            lines.push(`    ${oneLine(s.why)}`);
        if (s.id)
            lines.push(`    session ${s.id}`);
        if (s.path)
            lines.push(`    ${s.path}`);
    }
    return lines.join('\n') + '\n';
}
/**
 * The tools, as OpenCode's tool editor takes them: plain JSON Schemas, and
 * `codemode: false` so the model calls them directly instead of through
 * OpenCode's code-mode tool.
 */
export function hippoTools(config) {
    const run = (args, signal) => runHippo(args, { command: config.command, cwd: homedir(), timeoutMs: config.timeoutMs, signal });
    return [
        {
            name: 'hippo_ask',
            description: 'Search what coding agents on this machine already did (Claude Code, Codex, DeepSeek Harness and OpenCode sessions, '
                + 'indexed by hippo) and the workspace notes, for a question in plain words. Returns ranked past sessions and notes, '
                + 'each with why it matched and its session id or note path. Use it before solving something an agent may have solved before. '
                + 'Answer from the sources and cite each by its session id and date, or its note path. '
                + 'Read-only: hippo calls no model; the result becomes part of this session.',
            input: {
                type: 'object',
                properties: {
                    question: { type: 'string', description: 'What to look for, in plain words' },
                },
                required: ['question'],
                additionalProperties: false,
            },
            options: { codemode: false },
            execute: async (input, context) => ({ content: renderAsk(await run(askArgs(input), context.signal)) }),
        },
        {
            name: 'hippo_day',
            description: "Show what the coding agents on this machine did on a day, from hippo's index as of its last ingest: "
                + "the day's sessions, how each ended, the corrections, and its pull requests. Today in this machine's time zone "
                + 'unless a date is given. Read-only.',
            input: {
                type: 'object',
                properties: {
                    date: { type: 'string', pattern: '^\\d{4}-\\d{2}-\\d{2}$', description: 'The day, YYYY-MM-DD; today when left out' },
                },
                additionalProperties: false,
            },
            options: { codemode: false },
            execute: async (input, context) => ({ content: await run(dayArgs(input), context.signal) }),
        },
    ];
}
