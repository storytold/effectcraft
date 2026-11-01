#!/usr/bin/env bash
# Validate the plug-in catalog: schema, unique ids, every release's download (size + SHA-256),
# then install every package with the real EffectCraft loader.
#
#   scripts/validate.sh [index.json]
#
# Needs bash, jq, curl, sha256sum and npx (Node). Environment:
#   EFFECTCRAFT_CLI  path to an effectcraft-cli build; unset: the loader step is skipped.
#   SKIP_SCHEMA=1    skip the schema step (testing with a local catalog that names file:// URLs).
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
catalog=${1:-$here/index.json}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() { echo "error: $*" >&2; exit 1; }
# A path as the CLI sees it (Git Bash on Windows: /tmp/x -> C:/.../x).
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf %s "$1"; fi; }
# A file:// URL for a local path (file:///C:/x or file:///tmp/x).
file_url() {
  local p
  p=$(native "$1")
  [[ $p == /* ]] || p="/$p"
  printf 'file://%s' "$p"
}

echo "== 1. schema"
[ "$(wc -c <"$catalog")" -le $((4 << 20)) ] || fail "the catalog is larger than 4 MB (the app's limit)"
# Exactly one object of format 1 with a packages array (an empty file or a second value fails too),
# checked even when the schema step is skipped.
jq -e -s 'length == 1 and (.[0] | type == "object" and .schema == 1 and (.packages | type == "array"))' "$catalog" >/dev/null \
  || fail "$catalog isn't one JSON object with \"schema\": 1 and a \"packages\" array"
if [ "${SKIP_SCHEMA:-}" = 1 ]; then
  echo "skipped (SKIP_SCHEMA=1)"
else
  npx --yes ajv-cli@5.0.0 validate --spec=draft2020 --strict=false -s "$here/schema.json" -d "$catalog"
fi

echo "== 2. unique ids"
dups=$(jq -r '.packages[].id' "$catalog" | sort | uniq -d)
[ -z "$dups" ] || fail "duplicate package ids: $dups"
echo "ok"

echo "== 3. downloads"
# The checked files are what the loader step installs: a copy of the catalog names them by file://.
local_catalog=$(jq '.' "$catalog")
while IFS=$'\t' read -r p r id version url sha size; do
  file="$work/$id-$version.wasm"
  echo "$id $version: $url"
  curl -fsSL --retry 2 --max-filesize $((64 << 20)) -o "$file" "$url" || fail "$id $version: download failed"
  got_size=$(wc -c <"$file" | tr -d ' ')
  got_sha=$(sha256sum "$file" | cut -d' ' -f1)
  [ -z "$size" ] || [ "$size" = "$got_size" ] || fail "$id $version: size is $got_size, the catalog says $size"
  [ "$got_sha" = "$(tr 'A-F' 'a-f' <<<"$sha")" ] || fail "$id $version: SHA-256 is $got_sha, the catalog says $sha"
  local_catalog=$(jq --argjson p "$p" --argjson r "$r" --arg u "$(file_url "$file")" '.packages[$p].versions[$r].url = $u' <<<"$local_catalog")
done < <(jq -r '.packages | to_entries[] | .key as $p | .value as $pkg | $pkg.versions | to_entries[]
  | [$p, .key, $pkg.id, .value.version, .value.url, .value.sha256, (.value.size // "")] | @tsv' "$catalog")
echo "ok"

echo "== 4. load with EffectCraft"
if [ -z "${EFFECTCRAFT_CLI:-}" ]; then
  echo "skipped (set EFFECTCRAFT_CLI to an effectcraft-cli build)"
  exit 0
fi
printf '%s\n' "$local_catalog" >"$work/index.json"
mkdir -p "$work/config"
export EFFECTCRAFT_CONFIG_DIR=$(native "$work/config")
"$EFFECTCRAFT_CLI" exec library.refresh "{\"catalog\": \"$(file_url "$work/index.json")\", \"wait\": true}"
# shortcut: installs each package's newest release this build runs, not every release; older releases were checked when they were added.
for id in $(jq -r '.packages[].id' "$catalog"); do
  "$EFFECTCRAFT_CLI" exec library.install "{\"id\": \"$id\", \"wait\": true}" || fail "$id didn't install"
done
echo "ok"
