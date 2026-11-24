#!/bin/sh
set -eu

if [ -z "${EFFECTCRAFT_CONFIG_DIR:-}" ]; then
  export EFFECTCRAFT_CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/effectcraft-nightly"
fi
real_bin="$(dirname "$0")/.effectcraft-nightly/effectcraft"
exec "$real_bin" "$@"
