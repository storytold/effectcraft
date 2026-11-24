#!/bin/sh
set -eu

real_bin="$(dirname "$0")/.effectcraft-nightly/effectcraft"
exec "$real_bin" "$@"
