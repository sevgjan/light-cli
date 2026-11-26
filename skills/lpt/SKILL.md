---
name: lpt
description: Submit and manage Light Phone (LP3) tools with the `lpt` CLI. Register a tool on the Light dashboard, upload screenshots, pick tags, submit a build, watch or retry it, and download the signed APK. Use when the user wants to publish, submit, update or check the status of a tool in the Light Tool Library.
---

# lpt: Light Phone Tools CLI

`lpt` drives the same (undocumented) API as dashboard.thelightphone.com. It is unofficial; if a call fails in a way that looks like the API changed, say so and point the user to the dashboard.

Run it from inside the tool's git repo (it needs `tool/lighttool.toml` and an `origin` remote). `lpt --help` and `lpt <command> --help` list everything.

## Ground rules

- **Never ask for or handle the user's Light password.** If `lpt whoami` says not signed in, ask the user to run `lpt login` themselves in their terminal (in Claude Code, typing `! lpt login` runs it inside the session). Never read `~/.config/lpt/credentials.json`, never print tokens.
- **`create`, `edit`, `images rm`, `submit` and `retry` change things on Light's servers.** Show the user exactly what will be sent (title, description, tags, git ref, changelog, images) and get a clear yes before running them with `-y`. Without `-y` the commands prompt, which an agent cannot answer.
- Reads are safe any time: `whoami`, `list`, `show`, `tags`, `builds`, `images list`, `check`. Add `--json` when you need to parse the result.
- Only one build can be active per tool, and Light reviews approved builds by hand afterwards (they email the developer).

## Submit a new tool

1. `lpt whoami`. Developer mode must be on (the dashboard: Settings, Account, Developer Account; it needs an email confirmation).
2. `lpt check`. Fix every FAIL: LICENSE, `sdkVersion` in `gradle.properties`, clean tracked files, `serverPackage = "com.lightos"`, plain `x.y.z` versionName, and (when signed in to a registered tool) a `versionCode` higher than Light's latest build. Warnings (README sections, older SDK) are worth fixing too. The commit must be pushed to GitHub: Light builds from the repo, not from the working tree.
3. `lpt tags --json`, then ask the user which tags apply (zero or more).
4. Agree title and description with the user. Defaults are the `label` in `lighttool.toml` and the first paragraph of the README.
5. After approval: `lpt -y create --title "..." --description "..." --tag "Name" --tag "Other"`.
6. Screenshots: at least one is required for approval. Look for a `screenshots/` folder; if there is none, tell the user to capture some (the LP3 simulator or the phone) rather than inventing images. PNG, JPEG or WebP: `lpt images add shot1.png shot2.png` (the order is the display order). Check with `lpt images list`.
7. After approval of the changelog: `lpt -y submit -m "Initial release" --wait`. It re-runs preflight, refuses if HEAD is not pushed or `versionCode` is not higher than Light's latest build, then polls the build.
8. Report the result. When the build succeeds the version shows as pending approval; `lpt download` fetches the signed APK for sideload testing.

## Ship an update

Bump `versionCode` (strictly higher than Light's latest build, even while nothing is published: a resubmission at the same code fails preflight at once with `version_conflict`) and `versionName` in `tool/lighttool.toml`, set `sdkVersion` in `gradle.properties` to the current light-sdk release, commit, **push**, then `lpt check` and `lpt -y submit -m "<changelog>" --wait` after the user approves.

## When a build fails

`lpt builds` lists builds; `lpt builds <id>` prints the error log. Fix the cause, commit and push, and submit again (a new ref). Use `lpt retry` only when the failure was on Light's side (same ref, e.g. infrastructure), not for code errors. The error's `code` tells them apart: `version_conflict` means bump `versionCode`, commit, push and submit again; an `internal` category such as `signing_failed` ("Contact Light") is worth one `lpt retry` first.

## Status and reattaching

`lpt list` shows all tools with latest version and status. `lpt show` shows one tool's versions, builds and images. `lpt wait` reattaches to the latest build after an interrupted `submit --wait`; do not resubmit just to watch.

## Pointers

Select another tool with `--tool <id|title>` (the default is the tool whose repo matches this checkout). Tool rules and the review criteria: https://github.com/lightphone/light-sdk/blob/main/TOOL_GUIDELINES.md
