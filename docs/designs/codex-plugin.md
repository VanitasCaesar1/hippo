# A Codex plugin

Status: accepted, to build in 0.7.1. Amends [codex-hooks.md](codex-hooks.md), which turned down a Codex plugin when its only job would have been the three hooks.

## Background

hippo already works in Codex from a terminal: `hippo hooks install --agent codex` adds a session-start, a prompt and a session-end hook to `$CODEX_HOME/hooks.json` ([codex-hooks.md](codex-hooks.md)), and the Codex adapter indexes Codex's sessions. What Codex users lack is what Claude Code users get from hippo's Claude Code plugin: a way in from inside the agent. Someone who lives in the Codex app has no `/hippo:setup`, and has to find hippo's commands themselves.

Codex has plugins: a folder with a manifest that can carry skills, MCP servers, apps and hooks, installed from a *marketplace*, which is a Git repository or a local folder holding a marketplace file. Both the Codex app (Plugins › Add a marketplace) and the CLI (`codex plugin marketplace add`, `codex plugin add`) install from one.

## What changes for a user

In the Codex app: Plugins › **Add a marketplace**, source `VanitasCaesar1/hippo`, then **+** on hippo. From a terminal:

```sh
codex plugin marketplace add VanitasCaesar1/hippo
codex plugin add hippo@hippo
```

The plugin adds one skill, `$hippo`. Its default prompts, which the app shows on the plugin's card, are three things a person would type: *Set up hippo for this project*, *Has any agent already solved the flaky upload test?*, *What's waiting in my hippo Inbox?*. The skill:

- **Sets hippo up**: finds the binary (`PATH`, `~/.hippo/bin-path`, `~/.local/bin`, `~/.cargo/bin`); if there is none, shows the one-line install, says what it does and runs it only on a yes; then the notes folder (`hippo init`), Codex's hooks (`hippo hooks install --agent codex`, with the reminder that Codex runs them once trusted in its review), the first index (`hippo ingest`), and the nightly schedule (`hippo schedule install`) only after an explicit yes.
- **Asks**: `hippo ask --sources-only`, ranked past sessions and notes with no model call; Codex answers from them and cites them.
- **Briefs**: `hippo brief --task`, the start block for a task.
- **Walks the Inbox**: `hippo lessons`, then `hippo lessons keep` or `dismiss` for the lessons the person names, never on its own.

Nothing in hippo itself changes for this. The plugin carries no hooks and no MCP server; the hooks stay where `hippo hooks install --agent codex` puts them, and `hippo mcp install --client codex` stays the way to add hippo's MCP tools.

## How Codex reads a plugin

Read from Codex's source at commit `076ac78` (20 November 2026) and checked against `codex-cli 0.170.0`:

- **The marketplace file.** A marketplace root is searched for, in order, `.agents/plugins/marketplace.json`, `.agents/plugins/api_marketplace.json`, `.claude-plugin/marketplace.json` and `.cursor-plugin/marketplace.json`; the first that exists is used (`codex-rs/core-plugins/src/marketplace.rs`, `MARKETPLACE_MANIFEST_RELATIVE_PATHS`). Each plugin entry has a `name`, a `source` (`{"source": "local", "path": "./…"}`, a Git URL, a Git subdirectory or an npm package), a `policy` (`installation`: `AVAILABLE`, `INSTALLED_BY_DEFAULT` or `NOT_AVAILABLE`; `authentication`: `ON_INSTALL` or `ON_USE`) and a `category`. A local `path` must start with `./` and stay inside the root.
- **The manifest.** `<plugin>/.codex-plugin/plugin.json`, else `<plugin>/.claude-plugin/plugin.json`. Fields: `name`, `version`, `description`, `keywords`, `skills`, `mcpServers`, `apps`, `hooks`, and `interface` with `displayName`, `shortDescription`, `longDescription`, `developerName`, `category`, `capabilities`, `websiteURL`, `privacyPolicyURL`, `termsOfServiceURL`, `defaultPrompt`, `brandColor`, `composerIcon`, `logo`, `logoDark` and `screenshots` (`codex-rs/core-plugins/src/manifest.rs`, `RawPluginManifest`). Every path must start with `./`, must not contain `..`, and must resolve inside the plugin's folder, or it is ignored with a warning. `defaultPrompt` keeps at most 3 prompts of at most 512 characters.
- **Defaults.** With no `skills`, Codex reads `<plugin>/skills/`; with no `hooks`, it reads `<plugin>/hooks/hooks.json` if it exists; with no `mcpServers`, `<plugin>/.mcp.json` (`codex-rs/core-plugins/src/loader.rs`, `DEFAULT_*`).
- **Skills.** A folder with a `SKILL.md` whose frontmatter has `name` (at most 64 characters) and `description`, and optionally `agents/openai.yaml` with an `interface` (`display_name`, `short_description`, `default_prompt`, `brand_color`, icons) and a `policy` (`allow_implicit_invocation`, true by default) (`codex-rs/ext/skills/src/loader/mod.rs`; the skill-creator sample's `references/openai_yaml.md`). A plugin's skill is listed as `<plugin>:<skill>`; the person invokes it as `$hippo`.
- **Install.** `codex plugin add` copies the plugin's folder to `$CODEX_HOME/plugins/cache/<marketplace>/<plugin>/<version>/`.

## The layout

```
.agents/plugins/marketplace.json     the Codex marketplace: one plugin, source ./plugins/codex
plugins/codex/
  .codex-plugin/plugin.json          the manifest
  assets/icon.png                    256 px, from brand/hippo-icon.svg
  skills/hippo/SKILL.md              the $hippo skill
  skills/hippo/agents/openai.yaml    its display name and default prompt
```

Three choices follow from how Codex reads a plugin:

- **`.agents/plugins/marketplace.json` must exist.** Without it Codex falls back to `.claude-plugin/marketplace.json`, which names the Claude Code plugin in `plugin/`, and would install that into Codex, with a session-start hook that prints Claude Code's output, not Codex's.
- **The plugin has its own folder.** A plugin rooted at the repository root would copy the whole repository (the engine, the app, fixtures) into each user's Codex cache, and rooted at `plugin/` it would pick up the Claude Code plugin's `hooks/hooks.json` by default.
- **No hooks, no MCP server.** hippo's Codex hooks come from `hippo hooks install --agent codex`, which `hippo doctor` and `hippo hooks status` already check. Hooks inside a plugin would run a second copy of each hook for anyone who also ran that command. An MCP server in the manifest would start `hippo mcp` in every Codex session, including on machines where hippo is not installed yet, where it fails.

The plugin's version is the workspace's; `scripts/release.sh` refuses a release where they differ.

## Tests

- `crates/hippo/tests/plugins.rs` reads both plugins as their hosts do: the Codex marketplace points at `plugins/codex`; the manifest has the fields above, every path it names starts with `./`, stays inside the plugin and exists, the icons are PNG files, there are 1 to 3 default prompts within the length limit, its version is the workspace's, and there is no `hooks/hooks.json` or `.mcp.json` in the folder; the skill is named `hippo`, its `openai.yaml` names `$hippo`, and every command it tells Codex to run is one hippo has. The Claude Code plugin's marketplace, manifest, hooks and skills get the same checks.
- `scripts/codex-e2e/plugin.sh <codex>`, by hand before a release that changes the plugin: in a scratch `HOME` and `CODEX_HOME`, `codex plugin marketplace add` of the checkout, `codex plugin add hippo@hippo`, `codex plugin list`, and `skills/list` through `codex app-server` (no model is called), which must list `hippo:hippo` from the plugin's cache with no load errors. Run on 20 November 2026 with `codex-cli 0.170.0`: installed and enabled, the skill listed, no errors.

## Alternatives

- **Hooks in the plugin** instead of `hippo hooks install --agent codex`. One install for everything, but the hooks would need trust anyway, would double for anyone who installed them the other way, and would run on machines without hippo. Rejected for now; the skill runs the existing command.
- **One plugin folder for Claude Code and Codex.** Codex reads `.claude-plugin/plugin.json`, so the Claude Code plugin would load. But its skills are written for Claude Code (`disable-model-invocation`, `allowed-tools`, `$ARGUMENTS`) and its hook for Claude Code's output. Rejected.
