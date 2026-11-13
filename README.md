# claude-session-limit

Tiny borderless always-on-top X11 widget (Linux Mint / Cinnamon) showing your
Claude Code 5-hour session usage:

```
Session  ███████████░░░░  71%
Reset    ██████████████░  0:39 @ 19:00
```

- Bar 1: % of the 5h session limit used (green / amber ≥70% / red ≥90%).
- Bar 2: elapsed part of the 5h window; time until reset (`h:mm`) and reset time (24h).
- A small red dot in the corner means the last refresh failed (showing stale data).

Uses the Claude CLI login (`~/.claude/.credentials.json`); it never refreshes the
token itself, so if the token expired just use `claude` once. Polls
`https://api.anthropic.com/api/oauth/usage` (an undocumented endpoint) every 60 s.

Memory: ~8 MB RSS.

## Controls

| Action | Effect |
| --- | --- |
| Left drag | Move window (position is remembered in `~/.config/claude-session-limit/pos`) |
| Right click / `r` / F5 | Refresh now |
| Middle click / `q` / Esc | Quit |

## Build & install

```sh
cargo build --release
install -Dm755 target/release/claude-session-limit ~/.local/bin/claude-session-limit
# start on login:
cp claude-session-limit.desktop ~/.config/autostart/
```

Env overrides: `CLAUDE_CONFIG_DIR` (default `~/.claude`), `CLAUDE_CREDENTIALS` (path to the credentials file).
