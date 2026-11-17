# Light Phone Tools CLI

`lpt` submits and manages Light Phone tools from the terminal instead of the dashboard. It includes an [agent skill](#use-with-an-ai-agent), so you can also just ask Claude Code or Codex to submit your tool.

> **Unofficial.** This talks to the JSON API behind dashboard.thelightphone.com, which Light has not documented. It can change without notice. Login, listing, creating tools, image upload and build submission have been used against the live service; the field names for versions and build statuses are inferred, so report anything that looks off.

## Install

Homebrew (macOS, Linux):

    brew install sevgjan/tap/lpt

Prebuilt binaries for macOS and Linux are on the [releases page](https://github.com/sevgjan/light-cli/releases). Or with Rust:

    cargo install --git https://github.com/sevgjan/light-cli

`cargo install` puts `lpt` in `~/.cargo/bin`; make sure that is on your PATH.

## Use

    lpt login                 # email + password, same as the dashboard
    lpt whoami
    lpt list                  # your tools with latest version and status
    lpt show                  # versions, builds, images for this repo's tool
    lpt tags

In a tool repo (needs `tool/lighttool.toml`, an `origin` remote on GitHub):

    lpt check                 # local preflight, no login needed
    lpt create --tag utilities
    lpt images add shot1.png shot2.png
    lpt submit --wait         # preflight, confirm, build, poll
    lpt builds [BUILD_ID]     # list, or one build with its error log
    lpt retry
    lpt download              # signed APK of the latest good build

Pick a different tool with `--tool <id|title>`; `--json` prints raw API data; `-y` skips prompts.

`submit` refuses when: preflight fails, HEAD isn't the tip of a branch on origin, `versionCode` isn't higher than the last version, or a build is still running. `--force` overrides the first three.

## Use with an AI agent

`lpt` ships an agent skill that teaches Claude Code and Codex (any agent that reads `SKILL.md`) the submit workflow: preflight, tags, images, build, retry. Install it once:

    lpt skill install                  # Claude Code (~/.claude/skills) and Codex (~/.agents/skills)
    lpt skill install --agent claude   # or --agent codex
    lpt skill install --project        # into the current repo instead of your home directory

Then ask your agent to "submit this tool with lpt". The skill has the agent confirm anything that changes your dashboard before it runs, and never handle your password: you sign in yourself with `lpt login`. The skill source is [`skills/lpt/SKILL.md`](skills/lpt/SKILL.md).

## Auth

The dashboard has no OAuth or API keys, so login is email and password. Only the returned token is stored, in `~/.config/lpt/credentials.json` (mode 0600). `LIGHT_TOKEN` overrides it; `LIGHT_EMAIL` and `LIGHT_PASSWORD` allow non-interactive login. `logout` revokes the token server-side.

## License

MIT
