/**
 * Running the `hippo` binary: arguments as an array (never a shell), a
 * timeout, and only its standard output and exit status read back.
 */
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, join } from 'node:path';
/** Why a hippo command did not give its output. */
export class HippoError extends Error {
    reason;
    constructor(message, reason) {
        super(message);
        this.name = 'HippoError';
        this.reason = reason;
    }
}
/**
 * PATH with the folders hippo's installer and `cargo install` use added
 * after it, as the Claude Code plugin's hook does.
 */
export function searchPath(path = process.env.PATH ?? '', home = homedir()) {
    const extra = [join(home, '.local', 'bin'), join(home, '.cargo', 'bin')];
    return [...path.split(delimiter).filter(Boolean), ...extra].join(delimiter);
}
/** Run `hippo <args>` and return its standard output. */
export function runHippo(args, options) {
    const shown = `hippo ${args.join(' ')}`;
    return new Promise((resolve, reject) => {
        execFile(options.command, [...args], {
            cwd: options.cwd,
            env: { ...process.env, PATH: searchPath() },
            timeout: options.timeoutMs,
            killSignal: 'SIGKILL',
            maxBuffer: 16 * 1024 * 1024,
            encoding: 'utf8',
            signal: options.signal,
            windowsHide: true,
        }, (error, stdout, stderr) => {
            if (error === null) {
                resolve(stdout);
                return;
            }
            if (error.name === 'AbortError') {
                reject(error);
                return;
            }
            const code = error.code;
            if (code === 'ENOENT' && !existsSync(options.cwd)) {
                reject(new HippoError(`${shown} could not start: its directory ${options.cwd} does not exist`, 'failed'));
                return;
            }
            if (code === 'ENOENT') {
                reject(new HippoError(`hippo is not installed: ${JSON.stringify(options.command)} is not on PATH, in ~/.local/bin or in ~/.cargo/bin. `
                    + 'Install it with: curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippo/main/install.sh | sh, then run hippo start.', 'missing'));
                return;
            }
            if (error.killed === true) {
                reject(new HippoError(`${shown} did not finish in ${options.timeoutMs} ms`, 'timeout'));
                return;
            }
            const said = firstLines(stderr || stdout);
            if (/not set up/.test(said)) {
                reject(new HippoError(said, 'not-set-up'));
                return;
            }
            const status = typeof code === 'number' ? `exit ${code}` : String(code ?? error.message);
            reject(new HippoError(`${shown} failed (${status})${said ? `: ${said}` : ''}`, 'failed'));
        });
    });
}
function firstLines(text, count = 5) {
    return text.trim().split('\n').slice(0, count).join('\n');
}
