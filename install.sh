#!/usr/bin/env bash
# Build and install onto a SteamOS box: ./install.sh deck@steamdeck.local
set -euo pipefail
host=${1:?usage: $0 user@host}
cd "$(dirname "$0")"

cargo build --release --target x86_64-unknown-linux-musl
bin=target/x86_64-unknown-linux-musl/release/steamos-ha-wake

ssh "$host" 'mkdir -p ~/.local/bin ~/.config/systemd/user ~/.config/steamos-ha-wake'
scp "$bin" "$host:.local/bin/steamos-ha-wake"
scp dist/steamos-ha-wake.service "$host:.config/systemd/user/"
ssh "$host" '[ -e ~/.config/steamos-ha-wake/config.toml ]' \
  || scp dist/config.example.toml "$host:.config/steamos-ha-wake/config.toml"

ssh "$host" 'systemctl --user daemon-reload && systemctl --user enable steamos-ha-wake && systemctl --user restart steamos-ha-wake'
echo "Installed. Edit ~/.config/steamos-ha-wake/config.toml on $host, then: systemctl --user restart steamos-ha-wake"
echo "Check:  ssh $host '~/.local/bin/steamos-ha-wake probe'"
