---
name: ask
description: Ask whether a coding agent on this machine already solved something, answered from hippo's ranked past sessions.
argument-hint: <question>
disable-model-invocation: true
allowed-tools: Bash(hippo ask *), Bash(hippo show session *)
---

The user's question: $ARGUMENTS

If the question is empty, ask the user what they want to look for and stop.

Run `hippo ask --sources-only '<question>'`, with the question as one shell-quoted argument (escape any single quote in it). `--sources-only` ranks past sessions and turns without calling a model, so nothing is sent anywhere.

- If `hippo` is not found or says it is not set up, tell the user to run `/hippo:setup` and stop.
- Otherwise read the ranked sources and answer the question from them alone. Cite each source you use by its session id and date as printed. If you need more of a session, run `hippo show session <id>` for it.
- If the sources do not answer the question, say so plainly and suggest other words to try. Do not fill the gap from your own knowledge without saying that is what you are doing.
