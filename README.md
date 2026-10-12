# PR Review and PR Bar

Two macOS apps for your GitHub pull requests. Install either or both.

**PR Bar** sits in the menu bar. It watches GitHub and sends the notifications.

- **Menu:** what needs you right now, in GitHub's terms, one icon per PR. Review requests: ○ waiting on you, 🔴 its CI is failing, ✅ you approved (kept for a while so you can merge), oldest first. Your own PRs: ✅ ready to merge now (approved, checks green, no conflicts), ❌ changes requested, 🔴 CI failing, ⚠️ merge conflicts, ○ anything else (awaiting review, CI running, branch behind). Just merged: 🟣; just closed: ⚫. The badge counts review requests.
- **Each PR** has a submenu: Open in PR Review, Open on GitHub, Copy review command, Ignore. "Copy review command" copies your review command for that PR (e.g. `/chriscole-skills:pr-review <url>`), ready to paste into Claude Desktop or a terminal.
- **Notifications:** new review requests, CI results and reviews on your PRs, and new PRs in watched repos. Each kind can be switched on or off.

**PR Review** is the desktop app: a review UI backed by Claude.

- **Explain changes:** Claude reads the diff and the code around it. You get a summary and a risk rating, the diff retold as chapters in reading order, and inline notes on what each hunk does and why.
- **Run pr-review:** runs the [`pr-review` skill](https://github.com/ChrisColeAttensi/claude-skills) in a throwaway git worktree at the PR head. It never posts, pushes or fixes anything, and the report lands in the app. It can live-check the change in your Chrome if you tick the box.
- **Claude tab:** carry on the review conversation, e.g. "fix the findings", "turn my notes into review comments". **Continue in Claude Desktop** opens the same session in Desktop's Code tab.
- **Act on it:** approve, request changes, comment or merge from the app. Merge includes the admin override when your repo rules let you bypass. Afterwards, the app offers to DM the author on Slack.
- **Private notes:** click any line number to leave a note. Notes stay on your machine.
- **Triage:** filters for needs action only, team requests and drafts. Right-click a PR to ignore it.

## Install

### 1. Prerequisites

PR Bar on its own needs only the first two below.

You need an **Apple Silicon Mac** and Homebrew. Then:

1. **Repo access:** read access to the private [`homebrew-tap`](https://github.com/ChrisColeAttensi/homebrew-tap) repo. It holds the install recipe and the built app, so that's all installing needs. Ask Chris to add you. This repo, the source, is only needed if you want to work on the app.
2. **GitHub CLI:** install it and sign in with the account that has that access:
   ```bash
   brew install gh
   ```
   ```bash
   gh auth login
   ```
3. **Claude Code CLI:** install it, then run `claude` once and type `/login`:
   ```bash
   curl -fsSL https://claude.ai/install.sh | bash
   ```
4. **The pr-review skill**, from Chris's public skills repo:
   ```bash
   claude plugin marketplace add ChrisColeAttensi/claude-skills
   ```
   ```bash
   claude plugin install chriscole-skills@chriscole
   ```

### 2. Install the apps

```bash
brew tap chriscoleattensi/tap https://github.com/ChrisColeAttensi/homebrew-tap
```
```bash
brew trust chriscoleattensi/tap
```
```bash
brew install --cask chriscoleattensi/tap/pr-bar
```
```bash
brew install --cask chriscoleattensi/tap/pr-review
```

Homebrew 7+ only installs from taps you have trusted, which is what `brew trust` is for.

Open **PR Bar** from Applications. It appears in the menu bar and starts at login from then on; switch that off in its Settings. Open **PR Review** whenever you want the full review UI.

The cask downloads the build from a private release on `homebrew-tap`, using your `gh` login, or `HOMEBREW_GITHUB_API_TOKEN` if that's set. The app isn't signed with an Apple Developer ID, so the cask clears macOS's download quarantine flag for you.

### 3. Set it up

Open **Settings** (⌘,):

- **Repositories:** pr-review and Explain work best with a local clone of the repo. The app finds clones under `~/Developer` by default. Add other folders, or map a repo to a path by hand.
- **Monitoring:** choose what to show and what notifies you.
- **Slack** (optional): connect Slack in claude.ai → Settings → Connectors. Check that `claude mcp list` shows `claude.ai Slack … Connected`, then use **Send test message to myself**. Messages go out as you. No Slack app or admin approval is needed.
- **Live check in Chrome** (optional): needs the [Claude in Chrome](https://claude.ai/chrome) extension.

### Update

```bash
brew update && brew upgrade --cask pr-bar pr-review
```

### Uninstall

```bash
brew uninstall --zap --cask pr-review pr-bar
```

`--zap` on PR Review also removes the shared data (settings, reports, notes). On PR Bar it only removes its login item.

## Using it

| Where | What |
| --- | --- |
| Menu bar (PR Bar) | Hover a PR for Open in PR Review, Open on GitHub, Copy review command, Ignore. |
| Sidebar | `j` / `k` to move, `/` to filter, right-click for Ignore, Open on GitHub, Explain, Run pr-review. |
| PR tabs | Overview · Walkthrough · Files · Claude (pr-review + chat) · My notes (`⌘1`–`⌘5`) |
| Header | Review… (approve / request changes / comment), Merge…, Explain, Run pr-review. Buttons show a time and cost estimate learned from your own past runs. |

Claude runs use **your** `claude` login and plan, so they count against your usage. The time and cost estimates are learned from your own past runs.

## Develop

```bash
bun install
```
```bash
bun run tauri dev
```

That runs PR Review. For PR Bar, while that is running (it serves the frontend):

```bash
cargo run -p pr-bar
```

`bun run dev` serves the UI in a normal browser with a mocked backend. It reads fixtures from `src/dev/fixtures/`, which is gitignored.

Layout: `core/` is the Rust library both apps share (GitHub, settings, local PR state). `src-tauri/` is PR Review, `bar/src-tauri/` is PR Bar, and `src/` is the frontend for both (`bar.html` is PR Bar's settings window).

### Windows

The app also builds on Windows. It needs the Rust MSVC toolchain and bun; WebView2 ships with Windows 11.

```powershell
$env:TAURI_APP_PATH = "src-tauri"; bun run tauri build
```

This produces a per-user NSIS installer (no admin rights needed) in `target/release/bundle/nsis/`. Use `bar/src-tauri` for PR Bar. It isn't code-signed, so SmartScreen warns on first run: choose **More info → Run anyway**. `gh`, `git` and `claude` must be on `PATH`. Use Claude Code's native installer (`claude.exe`) rather than the npm one, or set the path to `claude` in Settings. The release script and Homebrew cask are macOS only.

### Release

Each app is released on its own:

```bash
scripts/release.sh review 0.4.0
```
```bash
scripts/release.sh bar 0.1.0
```

This bumps that app's version and tags it here (`v…` or `bar-v…`), builds it, publishes the zip as a release on `homebrew-tap` (not on this repo, so installers never need access to the source), and points its cask at it. Users get it with `brew upgrade`.

## How it works

- **GitHub:** everything goes through `gh` (GraphQL search plus `gh pr …`), every 2 minutes by default.
- **Two apps, one data folder:** while PR Bar runs, it alone polls GitHub and notifies, and writes the list to `snapshot.json`; PR Review reads it. Without PR Bar, PR Review polls itself, without notifications. The apps signal each other through small files there (see `core/src/sync.rs`).
- **Claude:** headless `claude -p` runs. pr-review and Explain run in `worktrees/<owner>__<repo>-<n>`. Chat resumes the same session there. Uncommitted chat edits are stashed, never lost, if the worktree is re-checked out.
- **Slack:** a short Haiku run that may call only the Slack tool it needs. Matches are found by commit email and remembered.
- **Cleanup:** worktrees are removed once their PR closes, or after a retention period (Settings → Review worktrees).
- **Data:** `~/Library/Application Support/com.chriscole.prreview/` holds settings, notification state, per-PR reports, notes and chats, run history, Slack matches and worktrees.
