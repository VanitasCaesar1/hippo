# hippo in any MCP client

`hippo mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/specification/2025-11-25) server over stdio. Any agent that can start a local MCP server can ask hippo for the start block, search past sessions, and see the day, without a hippo adapter for that agent. The design is in [`docs/designs/mcp-server.md`](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/mcp-server.md).

## Set it up

You need an index first (`hippo start`, or `hippo ingest` once hippo is set up). `hippo_context` also needs a notes folder (`hippo init`).

```sh
hippo mcp install --client cursor            # prints the entry and the file it goes in
hippo mcp install --client cursor --write    # merges it into that file, after a backup
```

`--client` takes `cursor`, `claude-desktop`, `gemini`, `windsurf`, `zed`, `vscode` or `claude-code`. `--write` keeps every other server and setting in the file in their order, replaces only an earlier `hippo` entry, and copies the old file to `<file>.hippo-backup-<UTC time>` first. When the file holds comments or is otherwise not plain JSON, `--write` leaves it alone and says so; add the printed entry by hand. Restart the client afterwards.

Add `--allow-write` to configure `hippo mcp --allow-write`, which also offers `hippo_note_correction` (below).

## Tools

| Tool | What it returns |
|---|---|
| `hippo_context` | The start block for a task: the required notes, then the notes relevant to `task`, within the byte budget. The text of `hippo context --task`. Pass `cwd`, the directory the agent works in, so notes about that repository rank higher. |
| `hippo_recall` | Matching turns from every indexed session: the text of `hippo recall <query>`. |
| `hippo_ask` | Past sessions and notes ranked for a question, each with why it matched and how to resume it: the text of `hippo ask --sources-only`. No model call; the client is the model. |
| `hippo_day` | A day's sessions, how each ended, and its events: the text of `hippo show day`. |
| `hippo_incidents` | What went wrong in the last days, which agent, at which step: the text of `hippo incidents`. |
| `hippo_note_correction` | Only with `--allow-write`. Takes the person's words (`said`) and a `rule`; hippo's lesson reviewer checks the rule in a fresh context, and a confirmed rule waits in the Inbox (`hippo lessons`, Hippo.app) until you keep or dismiss it. It never writes a note. |

Everything a tool returns is redacted first, with the patterns ingest uses (keys, tokens, `key=value` secrets, emails, phone numbers), because the client sends it on to a model. A note's email address therefore arrives as `[redacted:email]`.

## Client files

What `hippo mcp install` writes, per client, with the public documentation each comes from (read in November 2026). `<hippo>` is the absolute path of the `hippo` binary: the `hippo` on your `PATH` when it is the binary you ran, so a Homebrew link stays valid across upgrades.

### Cursor

File: `~/.cursor/mcp.json` (a project can have `.cursor/mcp.json`). Servers go under `mcpServers`. Cursor's field table lists `type` as required while its examples leave it out, so hippo writes it.

```json
{ "mcpServers": { "hippo": { "type": "stdio", "command": "<hippo>", "args": ["mcp"] } } }
```

Source: [Cursor, Model Context Protocol](https://cursor.com/docs/context/mcp).

### Claude Desktop

File: `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `~/.config/Claude/claude_desktop_config.json` on Linux (`%APPDATA%\Claude\claude_desktop_config.json` on Windows). Servers go under `mcpServers`. Quit and reopen the app to load it; its MCP logs are in `~/Library/Logs/Claude/`.

```json
{ "mcpServers": { "hippo": { "command": "<hippo>", "args": ["mcp"] } } }
```

Source: [Model Context Protocol, Connect to local MCP servers](https://modelcontextprotocol.io/quickstart/user).

### Gemini CLI

File: `~/.gemini/settings.json` (a project can have `.gemini/settings.json`). Servers go under `mcpServers`; a `command` makes it a stdio server. `gemini mcp add -s user` does the same from Gemini's side.

```json
{ "mcpServers": { "hippo": { "command": "<hippo>", "args": ["mcp"] } } }
```

Source: [Gemini CLI, MCP servers](https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md).

### Windsurf

File: `~/.codeium/windsurf/mcp_config.json`. Servers go under `mcpServers`. Windsurf's documentation now also names `~/.config/devin/mcp_config.json` as the file its "Open MCP config" action opens; if your build reads that one, copy the printed entry there.

```json
{ "mcpServers": { "hippo": { "command": "<hippo>", "args": ["mcp"] } } }
```

Source: [Windsurf, Cascade MCP](https://docs.devin.ai/windsurf/plugins/cascade/mcp).

### Zed

File: `~/.config/zed/settings.json` (a project can have `.zed/settings.json`). Servers go under `context_servers`. Zed's settings often hold comments; `--write` then refuses, and you paste the entry with `zed: open settings`.

```json
{ "context_servers": { "hippo": { "command": "<hippo>", "args": ["mcp"], "env": {} } } }
```

Sources: [Zed, Model Context Protocol](https://zed.dev/docs/ai/mcp), [Zed, Configuring Zed](https://zed.dev/docs/configuring-zed).

### VS Code (GitHub Copilot)

File: `~/.copilot/mcp-config.json` (or `$COPILOT_HOME/mcp-config.json`), the format VS Code's documentation recommends for new configurations, with servers under `mcpServers`. The older per-profile `mcp.json` (opened with "MCP: Open User Configuration") uses `servers` instead; to use it, put the printed entry under `servers`.

```json
{ "mcpServers": { "hippo": { "type": "stdio", "command": "<hippo>", "args": ["mcp"] } } }
```

Source: [VS Code, MCP configuration reference](https://code.visualstudio.com/docs/agents/reference/mcp-configuration).

### Claude Code

Claude Code adds servers with its own command and keeps user-scope servers in `~/.claude.json`, a state file it rewrites while it runs. hippo prints the command and does not edit that file (`--write` is refused):

```sh
claude mcp add --transport stdio --scope user hippo -- <hippo> mcp
```

Claude Code already gets the start block from hippo's hooks (`hippo hooks install`); the server adds the other tools.

Source: [Claude Code, Connect Claude Code to tools via MCP](https://code.claude.com/docs/en/mcp).

## Protocol

stdio, one JSON-RPC message per line. Protocol versions 2024-11-05, 2025-03-26, 2025-06-18 and 2025-11-25; a client asking for another gets 2025-11-25. Tools only: no resources or prompts (the design says why). Each call reads the configuration and the index afresh, so a new ingest shows without restarting the client.

## Checking it by hand

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"sh","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"hippo_recall","arguments":{"query":"retry"}}}' \
  | hippo mcp
```
