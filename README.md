# PR Review

Menu-bar app that watches GitHub PRs, notifies you, and gives you a review UI backed by Claude.

- **Monitors** (each toggle-able, with separate notify toggles in Settings): review requested from me, new commits on PRs I reviewed, my own PRs (CI result, incoming reviews), and all PRs in watched repos. Menu-bar badge = PRs waiting on you.
- **Explain changes**: runs `claude -p` with a JSON schema. You get a summary, a risk rating, the diff reordered as chapters, and inline notes on hunks (what / why / concern / question) in the Walkthrough and Files tabs.
- **Run pr-review**: runs your `/chriscole-skills:pr-review` skill headless, in a throwaway git worktree at the PR head. It stops before step 8, so nothing is posted, pushed or fixed. The report shows up in the pr-review tab.
- **My notes**: click any line number to leave a private note. Notes stay on your machine; you can copy one, or copy them all as markdown.

## Requirements

- `gh` authenticated (`gh auth status`)
- `claude` CLI logged in (`claude` then `/login`)
- A local clone of each repo you want pr-review to run on, under a folder listed in Settings → Repositories (default `~/Developer`)

## Run

```bash
bun install
bun run tauri dev          # dev
bun run tauri build --bundles app   # → src-tauri/target/release/bundle/macos/PR Review.app
```

`bun run dev` alone serves the UI in a normal browser with a mocked backend. It reads fixtures from `src/dev/fixtures/` (gitignored).

## Launch at login

Settings → General → **Launch at login** registers a LaunchAgent; login launches start hidden in the menu bar (`--hidden`). Enable it from the built `.app`, not `tauri dev`, or it will point at the debug binary.

## Worktree cleanup

Each run checks the PR out into `worktrees/<owner>__<repo>-<n>`. About once an hour the app removes worktrees whose PR is merged or closed, and ones unused for longer than the retention setting (default 7 days; "Never" keeps them until the PR closes). Worktrees with a running job are skipped. Reports, walkthroughs and notes under `prs/` are kept. Settings → Review worktrees shows disk use and has **Clean up now** and **Remove all**.

## Slack

After you approve or merge someone else's PR, the app offers to DM the author. It matches them by the email on their commits, and shows who it found before you send. Messages go out as you through your **claude.ai Slack connector** (check `claude mcp list`), via a tiny Haiku run that may call only the Slack tool it needs. No Slack app or admin approval is needed. Confirmed matches are remembered in `slack-users.json`. Settings → Slack has the toggles, message templates, and a self-test.

## Keys

`j`/`k` next/prev PR · `/` filter · `⌘1–5` tabs · `⌘R` refresh · `⌘,` settings

## Data

`~/Library/Application Support/com.chriscole.prreview/`: `settings.json`, `seen.json` (notification state), `prs/<repo>-<n>/` (review.md, insights.json, drafts.json), `worktrees/`.
