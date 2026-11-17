# claude-session-limit

Tiny (112×36 px) borderless always-on-top X11 widget (Linux Mint / Cinnamon) showing your
Claude Code 5-hour session usage:

```
┌──────────────────┐
│ ███████83%██░░░░ │   5h session usage (green / amber ≥70% / red ≥90%)
│ ██0:33 @ 19:00█░ │   elapsed part of the 5h window, time to reset @ reset time
│ ██████7d 81%███░ │   only when another limit (weekly, per-model, ...) is ≥75%
└──────────────────┘
```

- Hover to see a tooltip with every limit, its usage and reset time.
- A small red dot in the corner means the last refresh failed (showing stale data).

Uses the Claude CLI login (`~/.claude/.credentials.json`); it never refreshes the
token itself, so if the token expired just use `claude` once. Polls
`https://api.anthropic.com/api/oauth/usage` (an undocumented endpoint) every 60 s.

Memory: ~8 MB RSS.

## Controls

| Action | Effect |
| --- | --- |
| Hover | Tooltip with all limits |
| Left drag | Move window (position is remembered in `~/.config/claude-session-limit/pos`) |
| Right click / `r` / F5 | Refresh now |
| Middle click | Quit |

## Build & install

```sh
cargo build --release
install -Dm755 target/release/claude-session-limit ~/.local/bin/claude-session-limit
# start on login:
cp claude-session-limit.desktop ~/.config/autostart/
```

Env overrides: `CLAUDE_CONFIG_DIR` (default `~/.claude`), `CLAUDE_CREDENTIALS` (path to the credentials file).
Set `CLAUDE_USAGE_DEBUG=1` to print the raw usage response to stderr.
