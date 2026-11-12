# Scheduling

On macOS, `hippocamp schedule` installs two launchd jobs for the current user. There is no background process of hippocamp's own: launchd starts the command, the command exits.

| Job label | Runs | When |
|---|---|---|
| `dev.hippocamp.ingest` | `hippocamp ingest` | Once when loaded, then every `ingest_interval_minutes` (10 by default), at background priority |
| `dev.hippocamp.dream` | `hippocamp dream` | Daily at `dream_at` (`03:30` by default). It does not run when loaded. |

## Install

```sh
hippocamp schedule install
```

```text
installed /Users/you/Library/LaunchAgents/dev.hippocamp.ingest.plist
installed /Users/you/Library/LaunchAgents/dev.hippocamp.dream.plist
```

For each job, `install` writes `~/Library/LaunchAgents/<label>.plist`, unloads any earlier version, and loads the new one with `launchctl bootstrap gui/<your user id>`. Running it again is safe and is how you apply a changed `ingest_interval_minutes` or `dream_at`.

The jobs run the absolute path of the `hippocamp` binary that ran `install`, with the environment of the shell that ran it:

- `PATH` and `HOME`, always;
- `HIPPOCAMP_HOME` and `CLAUDE_CONFIG_DIR`, when they are set and not empty. launchd starts jobs with almost no environment. Without `CLAUDE_CONFIG_DIR`, the dreaming job's `claude` would use the default profile, which may not be signed in.

Output of both jobs, standard output and standard error, goes to `$HIPPOCAMP_HOME/logs/<label>.log`.

If you install from a shell where `claude` is not on `PATH`, dreaming cannot find it. Install from the shell where you normally run `claude`.

## Check

```sh
hippocamp schedule status
```

```text
dev.hippocamp.ingest: launchctl print exit status 0
dev.hippocamp.dream: launchctl print exit status 0
```

`status` prints the exit status of `launchctl print` for each job. 0 means launchd knows the job. Any other number means it does not. `hippocamp doctor` has a `launchd` row that is a problem when either job's file is missing from `~/Library/LaunchAgents`.

## Uninstall

```sh
hippocamp schedule uninstall
```

Unloads both jobs and deletes their files. It prints `removed <path>` for each, or `<path> was not installed`.

## A nightly check

`hippocamp doctor` does not call the model unless you ask. A nightly check should run:

```sh
hippocamp doctor --model
```

It sends one word through `model_command` and reports a model that is signed out or unreachable. Its exit code is 1 on any problem. Without a schedule installed, a new setup passes `hippocamp doctor`: that row is `ok` and starts `not set up yet:`.

## The removable-volume prompt

If your workspace or your transcripts are on an external volume, macOS asks the first time a launchd job reads them: access to "files on a removable volume". Until someone answers, that run waits, and no other ingest starts, because ingest holds a lock. `hippocamp doctor` then reports a stale index and says a launchd job may be waiting on this prompt.

macOS stores the answer for the binary's path: a rebuilt `~/.cargo/bin/hippocamp` runs without asking again, but installing to a new path may ask again.

To get the prompt while you are at the Mac, run:

```sh
launchctl kickstart gui/$UID/dev.hippocamp.ingest
```

and click Allow.

## Other systems

On Linux and other systems, `hippocamp schedule install` writes nothing. It prints, for the same two jobs, a crontab block to add with `crontab -e` and a systemd user timer pair (a `.service` and a `.timer` for each job, to save under `~/.config/systemd/user/`), with the commands that enable the timers. Choose one. The jobs use the same binary path, environment, interval, time and log files as on macOS, and `--job ingest` or `--job dream` prints one job only. Without a notes folder it prints the ingest job only.

`hippocamp schedule status` prints the commands that check cron and systemd, `crontab -l | grep hippocamp` and `systemctl --user list-timers 'hippocamp-*'`, because hippocamp cannot see them. `hippocamp schedule uninstall` prints the commands that remove what you added. All three exit 0. Off macOS, `hippocamp doctor` cannot see cron or systemd either: an index older than three ingest intervals is a warning that names `hippocamp schedule`, not a problem.
