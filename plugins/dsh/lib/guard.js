/**
 * hippo's guard in dsh (`guard: true`): before every tool call, the plugin
 * runs `hippo guard check --agent dsh` with the call on standard input
 * (docs/designs/guard.md). A refusal becomes dsh's `deny`; a warning lets
 * the call run and goes to the model with the call's result, and to dsh's
 * log. In hippo 0.6 only hippo's self-protection rules refuse; everything
 * else warns.
 *
 * When hippo cannot answer (missing, failing, too slow), the call goes on,
 * unless `$HIPPO_HOME/guard/compiled.json` says `fail = "closed"`.
 */
import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { resolveCommand, searchPath } from "./hippo.js";
/** hippo's guard timeout is its own; this is the backstop. */
const BACKSTOP_MS = 5_000;
/** `fail` in the compiled rules: `closed` refuses a call hippo could not check. */
export function failMode(env = process.env, home = homedir()) {
    const hippoHome = env.HIPPO_HOME ? env.HIPPO_HOME : join(home, '.hippo');
    try {
        const compiled = JSON.parse(readFileSync(join(hippoHome, 'guard', 'compiled.json'), 'utf8'));
        return compiled.fail === 'closed' ? 'closed' : 'open';
    }
    catch {
        return 'open';
    }
}
function failed(why) {
    if (failMode() === 'closed') {
        return {
            decision: 'block',
            reason: `hippo guard could not check this call (${why}) and fail = "closed", so it is refused. Tell the person.`,
            failed: why,
        };
    }
    return { decision: 'none', failed: why };
}
/** Run `hippo guard check --agent dsh` on one call. */
export function checkCall(config, call, cwd) {
    return new Promise(resolve => {
        const child = execFile(resolveCommand(config.command), ['guard', 'check', '--agent', 'dsh'], {
            cwd,
            env: { ...process.env, PATH: searchPath() },
            timeout: Math.min(config.timeoutMs, BACKSTOP_MS),
            killSignal: 'SIGKILL',
            maxBuffer: 1024 * 1024,
            encoding: 'utf8',
            windowsHide: true,
        }, (error, stdout) => {
            if (error !== null) {
                resolve(failed(error.killed === true ? 'no answer in time' : error.message.split('\n')[0] ?? 'failed'));
                return;
            }
            try {
                const answer = JSON.parse(stdout.trim());
                if (!['none', 'warn', 'ask', 'block'].includes(answer.decision))
                    throw new Error('unknown decision');
                resolve(answer);
            }
            catch {
                resolve(failed('an answer that does not parse'));
            }
        });
        child.stdin?.on('error', () => { });
        child.stdin?.end(JSON.stringify(call));
    });
}
/** The message that carries a warning to the model with the call's result. */
export function guardMessage(reason) {
    const safe = reason.replaceAll('</system-reminder>', '<\\/system-reminder>');
    return {
        id: randomUUID(),
        role: 'user',
        content: [{ type: 'text', text: `<system-reminder>\n${safe}\n</system-reminder>` }],
        source: { kind: 'hippo-guard' },
    };
}
/** Add the guard's two listeners to `ctx`. */
export function addGuard(ctx, config) {
    const warnings = new Map();
    ctx.on('tools/pre-execute', async (exec, next) => {
        const cwd = exec.agent ? ctx.workingDirectory.get(exec.agent.session) : process.cwd();
        const answer = await checkCall(config, {
            tool_name: exec.name,
            tool_input: exec.arguments ?? null,
            cwd,
            session_id: exec.agent?.session.header.id ?? null,
            headless: false,
        }, cwd);
        if (answer.failed !== undefined)
            ctx.logger.warn(`hippo guard: ${answer.failed}`);
        if (answer.decision === 'block')
            return { kind: 'deny', reason: answer.reason ?? 'hippo guard refused this call' };
        if (answer.decision === 'ask')
            return { kind: 'ask', reason: answer.reason };
        if (answer.decision === 'warn' && answer.reason) {
            warnings.set(exec.callId, answer.reason);
            ctx.logger.warn(answer.reason);
        }
        return next();
    });
    ctx.on('tools/post-execute', async (exec, _result, next) => {
        const downstream = await next();
        const reason = warnings.get(exec.callId);
        if (reason === undefined)
            return downstream;
        warnings.delete(exec.callId);
        return { ...downstream, additionalContexts: [guardMessage(reason), ...(downstream.additionalContexts ?? [])] };
    });
}
