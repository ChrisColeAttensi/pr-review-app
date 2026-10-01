# PR Review

A macOS menu-bar app that watches your GitHub pull requests and gives you a review UI backed by Claude.

- **Menu bar:** what needs you right now. Review requests show how far along each one is (🛠 issues found, ✅ reviewed, ⏳ Claude working, ✦ explained, ○ not started), and your own PRs show their next step (🟢 ready to merge, 🟠 changes requested, 🔴 CI failing). The badge counts review requests.
- **Notifications:** new review requests, CI results and reviews on your PRs, and new PRs in watched repos. Each kind can be switched on or off.
- **Explain changes:** Claude reads the diff and the code around it. You get a summary and a risk rating, the diff retold as chapters in reading order, and inline notes on what each hunk does and why.
- **Run pr-review:** runs the [`pr-review` skill](https://github.com/ChrisColeAttensi/claude-skills) in a throwaway git worktree at the PR head. It never posts, pushes or fixes anything, and the report lands in the app. It can live-check the change in your Chrome if you tick the box.
- **Claude tab:** carry on the review conversation, e.g. "fix the findings", "turn my notes into review comments". **Continue in Claude Desktop** opens the same session in Desktop's Code tab.
- **Act on it:** approve, request changes, comment or merge from the app. Merge includes the admin override when your repo rules let you bypass. Afterwards, the app offers to DM the author on Slack.
- **Private notes:** click any line number to leave a note. Notes stay on your machine.
- **Triage:** filters for needs action only, team requests and drafts. Right-click a PR to ignore it.

## Install

### 1. Prerequisites

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

### 2. Install the app

```bash
brew tap chriscoleattensi/tap https://github.com/ChrisColeAttensi/homebrew-tap
```
```bash
brew trust chriscoleattensi/tap
```
```bash
brew install --cask chriscoleattensi/tap/pr-review
```

Homebrew 7+ only installs from taps you have trusted, which is what `brew trust` is for.

Open **PR Review** from Applications. It appears in the menu bar. Then turn on **Settings → General → Launch at login**.

The cask downloads the build from a private release on `homebrew-tap`, using your `gh` login, or `HOMEBREW_GITHUB_API_TOKEN` if that's set. The app isn't signed with an Apple Developer ID, so the cask clears macOS's download quarantine flag for you.

### 3. Set it up

Open **Settings** (⌘,):

- **Repositories:** pr-review and Explain work best with a local clone of the repo. The app finds clones under `~/Developer` by default. Add other folders, or map a repo to a path by hand.
- **Monitoring:** choose what to show and what notifies you.
- **Slack** (optional): connect Slack in claude.ai → Settings → Connectors. Check that `claude mcp list` shows `claude.ai Slack … Connected`, then use **Send test message to myself**. Messages go out as you. No Slack app or admin approval is needed.
- **Live check in Chrome** (optional): needs the [Claude in Chrome](https://claude.ai/chrome) extension.

### Update

```bash
brew update && brew upgrade --cask pr-review
```

### Uninstall

```bash
brew uninstall --zap --cask pr-review
```

`--zap` also removes the app's data and its login item.

## Using it

| Where | What |
| --- | --- |
| Menu bar | Click a PR to open it in the app. |
| Sidebar | `j` / `k` to move, `/` to filter, right-click for Ignore, Open on GitHub, Explain, Run pr-review. |
| PR tabs | Overview · Walkthrough · Files · pr-review · My notes · Claude (`⌘1`–`⌘6`) |
| Header | Review… (approve / request changes / comment), Merge…, Explain, Run pr-review. Buttons show a time and cost estimate learned from your own past runs. |

Claude runs use **your** `claude` login and plan, so they count against your usage. The time and cost estimates are learned from your own past runs.

## Develop

```bash
bun install
```
```bash
bun run tauri dev
```

`bun run dev` serves the UI in a normal browser with a mocked backend. It reads fixtures from `src/dev/fixtures/`, which is gitignored.

### Windows

The app also builds on Windows. It needs the Rust MSVC toolchain and bun; WebView2 ships with Windows 11.

```powershell
bun run tauri build
```

This produces a per-user NSIS installer (no admin rights needed) in `src-tauri/target/release/bundle/nsis/`. It isn't code-signed, so SmartScreen warns on first run: choose **More info → Run anyway**. `gh`, `git` and `claude` must be on `PATH`. Use Claude Code's native installer (`claude.exe`) rather than the npm one, or set the path to `claude` in Settings. The release script and Homebrew cask are macOS only.

### Release

```bash
scripts/release.sh 0.4.0
```

This bumps the version and tags it here, builds the app, publishes the zip as a release on `homebrew-tap` (not on this repo, so installers never need access to the source), and points the cask at it. Users get it with `brew upgrade`.

## How it works

- **GitHub:** everything goes through `gh` (GraphQL search plus `gh pr …`). The app polls every 2 minutes by default.
- **Claude:** headless `claude -p` runs. pr-review and Explain run in `worktrees/<owner>__<repo>-<n>`. Chat resumes the same session there. Uncommitted chat edits are stashed, never lost, if the worktree is re-checked out.
- **Slack:** a short Haiku run that may call only the Slack tool it needs. Matches are found by commit email and remembered.
- **Cleanup:** worktrees are removed once their PR closes, or after a retention period (Settings → Review worktrees).
- **Data:** `~/Library/Application Support/com.chriscole.prreview/` holds settings, notification state, per-PR reports, notes and chats, run history, Slack matches and worktrees.
