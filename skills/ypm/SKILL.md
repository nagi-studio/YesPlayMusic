---
name: ypm
description: Control the local YesPlayMusic player — read playback, pause/resume, change tracks, or seek to a requested time. Use when the user asks about the music that is playing or wants it changed.
---

# ypm — YesPlayMusic remote control

`ypm` is the YesPlayMusic CLI. It talks to whichever player is running on this
machine (desktop GUI or terminal TUI) over a local socket; nothing here touches
the network or needs credentials.

## Procedure

1. Pick the subcommand: `status` (what's playing), `pause`, `resume`,
   `toggle`, `next`, `prev`, or `seek <seconds>` for an absolute position.
2. Run it with `--json` and answer from the parsed output — the output is the
   truth about player state: `ypm status --json`.
3. After a mutation, report it as done; run a follow-up `ypm status --json`
   only when the user asked what is now playing.

## Reference

- **No player running**: the command prints
  `没有运行中的播放器（GUI 或 TUI 都不在线）` — relay that and suggest opening
  YesPlayMusic (GUI) or running `ypm` (TUI). Retrying without a player is a
  no-op.
- **Both players running**: `--gui` / `--tui` forces the target; by default
  ypm picks the running one itself.
- **`ypm update`** upgrades the ypm binary, not playback — run it only when
  the user asks to upgrade ypm.
- For any flag not listed here, `ypm --help` is authoritative.
