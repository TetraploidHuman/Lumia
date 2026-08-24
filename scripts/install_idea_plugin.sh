#!/usr/bin/env bash
# Install the Lumi IDEA plugin on the remote/backend host.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IDEA_CONFIG="${HOME}/.local/share/JetBrains/IntelliJIdea2026.2"
PLUGIN_ZIP="$(ls -t "$ROOT/editors/idea/build/distributions/lumi-idea-"*.zip 2>/dev/null | head -1)"

if [[ -z "$PLUGIN_ZIP" || ! -f "$PLUGIN_ZIP" ]]; then
  echo "Building plugin first..."
  (cd "$ROOT/editors/idea" && ./gradlew buildPlugin --offline)
  PLUGIN_ZIP="$(ls -t "$ROOT/editors/idea/build/distributions/lumi-idea-"*.zip | head -1)"
fi

BACKUP_DIR="$IDEA_CONFIG/_disabled_plugin_backups"
mkdir -p "$BACKUP_DIR"
for old in "$IDEA_CONFIG"/lumia-idea* "$IDEA_CONFIG"/lumi-idea; do
  [[ -e "$old" ]] || continue
  ts="$(date +%Y%m%d%H%M%S)"
  echo "Moving old plugin dir: $old -> $BACKUP_DIR/"
  mv "$old" "$BACKUP_DIR/$(basename "$old")-$ts"
done

echo "Installing $PLUGIN_ZIP -> $IDEA_CONFIG/"
python3 -c "import zipfile,sys; z=zipfile.ZipFile(sys.argv[1]); z.extractall(sys.argv[2])" \
  "$PLUGIN_ZIP" "$IDEA_CONFIG"

LUMI_BIN="$ROOT/target/release/lumi"
if [[ -x "$LUMI_BIN" ]]; then
  mkdir -p "${HOME}/.local/bin"
  ln -sfn "$LUMI_BIN" "${HOME}/.local/bin/lumi"
  echo "Linked ${HOME}/.local/bin/lumi -> $LUMI_BIN"
fi
chmod +x "$ROOT/scripts/lumi-run.sh" 2>/dev/null || true

echo "Done. Restart the IDEA backend to load org.lumi.idea $(basename "$PLUGIN_ZIP" .zip | sed 's/lumi-idea-//')."
