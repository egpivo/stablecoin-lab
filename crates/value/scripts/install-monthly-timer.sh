#!/usr/bin/env bash
# Install the monthly value-layer refresh as a systemd user timer (run once on wtwang-wsl).
#   bash crates/value/scripts/install-monthly-timer.sh          # install and enable
#   bash crates/value/scripts/install-monthly-timer.sh --dry-run
# Runs on the 5th of each month at 06:00 local time; Persistent=true catches up after downtime.
set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")" && pwd)/monthly-update.sh"
UNIT_DIR="$HOME/.config/systemd/user"
NAME="stablecoin-value-update"

service="[Unit]
Description=Monthly stablecoin-lab value layer refresh (opens a PR, never merges)

[Service]
Type=oneshot
Environment=PATH=$HOME/.cargo/bin:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=/bin/bash $SCRIPT
TimeoutStartSec=1h
"

timer="[Unit]
Description=Monthly stablecoin-lab value layer refresh timer

[Timer]
OnCalendar=*-*-05 06:00:00
Persistent=true

[Install]
WantedBy=timers.target
"

if [[ "${1:-}" == "--dry-run" ]]; then
  printf '== %s/%s.service\n%s\n== %s/%s.timer\n%s\n' "$UNIT_DIR" "$NAME" "$service" "$UNIT_DIR" "$NAME" "$timer"
  exit 0
fi

mkdir -p "$UNIT_DIR"
printf '%s' "$service" > "$UNIT_DIR/$NAME.service"
printf '%s' "$timer" > "$UNIT_DIR/$NAME.timer"
systemctl --user daemon-reload
systemctl --user enable --now "$NAME.timer"
systemctl --user list-timers "$NAME.timer" --no-pager
