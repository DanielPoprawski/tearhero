#!/usr/bin/env bash
# Build tearhero, install the binary and the Omarchy shell plugin, and migrate
# the old bare-QML bar module entry in shell.json to the plugin.
set -euo pipefail

cd "$(dirname "$0")"

BIN="$HOME/.local/bin/tearhero"
PLUGIN_DIR="$HOME/.config/omarchy/plugins/tearhero"
OLD_MODULE="$HOME/.config/omarchy/bar/modules/tearhero.qml"
SHELL_JSON="$HOME/.config/omarchy/shell.json"

echo "→ building"
cargo build --release
install -Dm755 target/release/tearhero "$BIN"

echo "→ validating plugin"
omarchy plugin validate plugin

echo "→ installing plugin to $PLUGIN_DIR"
rm -rf "$PLUGIN_DIR"
mkdir -p "$(dirname "$PLUGIN_DIR")"
cp -r plugin "$PLUGIN_DIR"

if [ -e "$OLD_MODULE" ]; then
  echo "→ removing old bar module $OLD_MODULE"
  rm -f "$OLD_MODULE" "$OLD_MODULE".bak.*
fi

if [ -f "$SHELL_JSON" ]; then
  cp "$SHELL_JSON" "$SHELL_JSON.bak.$(date +%s)"
  tmp="$(mktemp "$SHELL_JSON.XXXXXX")"
  # Rewrite {"id":"tearhero","type":"qml"} -> {"id":"tearhero"} in every bar
  # section; append to the right section if no entry exists anywhere.
  jq '
    def has_tearhero: [.bar.layout // {} | .[]? | .[]? | if type == "object" then .id else . end] | index("tearhero") != null;
    if has_tearhero then
      .bar.layout |= with_entries(.value |= map(
        if type == "object" and .id == "tearhero" then {id: "tearhero"} else . end))
    else
      .bar.layout.right = ((.bar.layout.right // []) + [{id: "tearhero"}])
    end
  ' "$SHELL_JSON" > "$tmp"
  mv "$tmp" "$SHELL_JSON"
  echo "→ shell.json updated (backup kept alongside)"
fi

echo "→ restarting shell"
omarchy restart shell
echo "done"
