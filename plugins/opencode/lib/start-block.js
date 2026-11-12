/**
 * The start block: hippo's required context, added to the system prompt of
 * every model request of a top-level session whose directory is inside
 * hippo's workspace. OpenCode builds the system prompt again for each
 * request and never stores it, so the block survives a compaction without
 * help and never enters the session's messages.
 */
import { realpath } from 'node:fs/promises';
import { homedir } from 'node:os';
import { sep } from 'node:path';
import { HippoError, runHippo } from "./hippo.js";
/**
 * The block hippo gives a session in `cwd`, or `undefined` when there is
 * none: no workspace, `cwd` not a local folder inside it, or nothing to give.
 */
export async function startBlock(cwd, config) {
    const run = (args, dir) => runHippo(args, { command: config.command, cwd: dir, timeoutMs: config.timeoutMs });
    const shown = JSON.parse(await run(['config', 'show', '--json'], homedir()));
    if (typeof shown.workspace !== 'string' || shown.workspace === '')
        return undefined;
    const [dir, workspace] = await Promise.all([realpath(cwd).catch(() => undefined), realpath(shown.workspace).catch(() => undefined)]);
    if (dir === undefined || workspace === undefined || !inside(dir, workspace))
        return undefined;
    const block = await run(['context', '--parts', 'required'], workspace);
    return block.trim() === '' ? undefined : block;
}
/** Whether `dir` is `root` or a folder under it; both already resolved. */
export function inside(dir, root) {
    return dir === root || dir.startsWith(root.endsWith(sep) ? root : root + sep);
}
/** The system part's text that carries `block`. */
export function startText(block) {
    const safe = block.replaceAll('</system-reminder>', '<\\/system-reminder>');
    return '<system-reminder>\n'
        + "hippo's start block for this workspace (`hippo context --parts required`):\n\n"
        + `${safe.trimEnd()}\n`
        + '</system-reminder>';
}
/**
 * Join the `context` hook. Each session is decided once per plugin
 * instance, at its first request: its text, or `null` for none. Requests
 * that arrive while hippo runs wait for the same answer.
 */
export async function addStartBlock(ctx, config, warn) {
    const decided = new Map();
    let warnedMissing = false;
    const decide = async (sessionID) => {
        try {
            const info = await ctx.session.get({ sessionID });
            // A sub-agent gets none, as a Claude Code sub-agent gets no SessionStart hook.
            if (info.parentID !== undefined && info.parentID !== null)
                return null;
            const cwd = info.location?.directory ?? ctx.location.directory;
            const block = await startBlock(cwd, config);
            return block === undefined ? null : startText(block);
        }
        catch (error) {
            const missing = error instanceof HippoError && error.reason === 'missing';
            if (!missing || !warnedMissing) {
                const why = error instanceof Error ? error.message : String(error);
                warn(`hippo: no start block for this session: ${why}`);
            }
            warnedMissing ||= missing;
            return null;
        }
    };
    await ctx.session.hook('context', async (event) => {
        // A hook that throws can stop the model step: never throw.
        try {
            let text = decided.get(event.sessionID);
            if (text === undefined) {
                text = decide(event.sessionID);
                decided.set(event.sessionID, text);
            }
            const block = await text;
            if (block !== null && !event.system.some(part => part.text === block)) {
                event.system.push({ type: 'text', text: block });
            }
        }
        catch (error) {
            warn(`hippo: no start block for this request: ${error instanceof Error ? error.message : String(error)}`);
        }
    });
}
