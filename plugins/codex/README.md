# hippo for Codex

The Codex plugin for [hippo](https://github.com/VanitasCaesar1/hippo): one skill, `$hippo`, that sets hippo up for a project, asks past sessions, prints a brief for a task and walks the Inbox.

In the Codex app: **Plugins** › **Add a marketplace** › `VanitasCaesar1/hippo`, then **+** on hippo. From a terminal:

```sh
codex plugin marketplace add VanitasCaesar1/hippo
codex plugin add hippo@hippo
```

Then ask Codex to *set up hippo for this project*. The plugin carries no hooks and no MCP server; the skill runs hippo's own commands, and installs hippo (after asking) when it is missing. How it is built and why: [docs/designs/codex-plugin.md](../../docs/designs/codex-plugin.md). MIT, like hippo.
