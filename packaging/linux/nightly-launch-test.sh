#!/usr/bin/env bash
# Validate the launcher inside the flat Nightly tar without building or opening EffectCraft.
#
# That launcher is the canonical shared template (craft-common-actions scripts/nightly-launcher.sh.in),
# which the nightly producer accepts byte for byte: it starts `effectcraft-bin` beside itself on the
# nightly-v1 profile under the app's package identity, answers a version query with one plain
# `--version`, and wraps every other argument, including a command line that already names a profile.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/unpacked"
mkdir -p "$stage"

# A local argv recorder stands in for the compiled GUI, which the launcher starts as `effectcraft-bin`.
cat >"$stage/effectcraft-bin" <<'RECORDER'
#!/bin/sh
printf '<%s>\n' "$@"
RECORDER
chmod 755 "$stage/effectcraft-bin"
cp "$HERE/nightly-launch.sh" "$stage/effectcraft"
chmod 755 "$stage/effectcraft"

# A fixed profile base keeps the recorded arguments exactly comparable.
export XDG_DATA_HOME="$work/xdg-data"
identity=ai.storyteller.effectcraft.nightly
profile() {
  printf '<%s>\n' --launch-profile nightly-v1 --profile-root "$XDG_DATA_HOME/$identity" -- "$@"
}
expect() {
  expected="$1"
  shift
  actual=$("$stage/effectcraft" "$@")
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  fi
}
expect "$(profile)"
expect "$(profile 'project with spaces.ec' '--other-name.ec')" 'project with spaces.ec' '--other-name.ec'
# Both version spellings reach the GUI as one plain `--version`: the template never forwards `-V`.
expect "$(printf '<%s>\n' --launch-profile nightly-v1 --profile-root "$XDG_DATA_HOME/$identity" --version)" --version
expect "$(printf '<%s>\n' --launch-profile nightly-v1 --profile-root "$XDG_DATA_HOME/$identity" --version)" -V
# A command line that already names a profile is wrapped like any other argument: the template has no
# pass-through, so its own profile root is the one that is used and nothing the caller wrote is dropped.
supplied=(--launch-profile nightly-v1 --profile-root "$work/launcher-root" --version)
expect "$(printf '<%s>\n' --launch-profile nightly-v1 --profile-root "$XDG_DATA_HOME/$identity" -- "${supplied[@]}")" "${supplied[@]}"
# HOME is the only other base, and the profile root is never optional.
home_run=$(env -u XDG_DATA_HOME HOME="$work/home" "$stage/effectcraft")
expected_home=$(printf '<%s>\n' --launch-profile nightly-v1 --profile-root "$work/home/.local/share/$identity" --)
if [[ "$home_run" != "$expected_home" ]]; then
  printf 'expected:\n%s\nactual:\n%s\n' "$expected_home" "$home_run" >&2
  exit 1
fi
if env -u XDG_DATA_HOME -u HOME "$stage/effectcraft" >/dev/null 2>&1; then
  echo 'a launcher without HOME must refuse rather than start with the stable profile' >&2
  exit 1
fi
# The template starts the GUI beside itself or nothing: a missing copy is not looked for elsewhere.
mv "$stage/effectcraft-bin" "$work/moved-aside"
for args in "" "--version"; do
  if "$stage/effectcraft" $args >/dev/null 2>&1; then
    echo 'the launcher started without its GUI beside it' >&2
    exit 1
  fi
done
mv "$work/moved-aside" "$stage/effectcraft-bin"

echo "PASS: the flat Nightly tar's launcher starts the GUI through the Nightly launch profile"
