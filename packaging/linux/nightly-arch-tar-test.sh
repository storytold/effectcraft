#!/usr/bin/env bash
# Validate the staged Arch launcher without building or opening EffectCraft.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/source"
dst="$work/dist"
base=effectcraft-0.0.1-nightly.1-linux-x86_64
mkdir -p "$stage/bin" "$dst" "$work/extract"

# A local argv recorder stands in for the compiled binaries. It never opens a UI.
cat >"$stage/bin/effectcraft" <<'RECORDER'
#!/bin/sh
printf '<%s>\n' "$@"
RECORDER
chmod 755 "$stage/bin/effectcraft"
cp "$stage/bin/effectcraft" "$stage/bin/effectcraft-cli"

bash "$HERE/nightly-arch-tar.sh" "$stage" "$work/stage" "$dst" "$base"
tar -xzf "$dst/$base.tar.gz" -C "$work/extract"
bin="$work/extract/$base/bin"
# Simulate the existing Arch bin-tree copy and public launcher rename.
mkdir -p "$work/installed"
cp -R "$bin" "$work/installed/bin"
bin="$work/installed/bin"
mv "$bin/effectcraft" "$bin/effectcraft-nightly"
mv "$bin/effectcraft-cli" "$bin/effectcraft-nightly-cli"

test ! -e "$stage/bin/.effectcraft-nightly"
cmp "$stage/bin/effectcraft" "$bin/.effectcraft-nightly/effectcraft"
cmp "$stage/bin/effectcraft-cli" "$bin/effectcraft-nightly-cli"
expect() {
  expected="$1"
  shift
  actual=$(PATH="$bin:$PATH" effectcraft-nightly "$@")
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  fi
}
expect '<>'
expect "$(printf '<%s>\n' 'project with spaces.ec' '--other-name.ec')" \
  'project with spaces.ec' '--other-name.ec'
expect '<--version>' --version
expect '<-V>' -V

echo "PASS: Arch Nightly handoff stages the hidden binary and preserves launch arguments"
