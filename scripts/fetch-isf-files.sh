#!/bin/bash
# Fetch the public Vidvox ISF-Files corpus (github.com/Vidvox/ISF-Files) into
# shaders/isf-files/ for `vlfo check`. Not committed; see that repo's license.
set -euo pipefail
cd "$(dirname "$0")/../shaders/isf-files"
gh api 'repos/Vidvox/ISF-Files/git/trees/master?recursive=1' --jq '.tree[].path' \
  | grep -E '^ISF/.*\.(fs|vs)$' | while read -r p; do
    f="$(basename "$p")"
    [ -f "$f" ] || curl -sfL "https://raw.githubusercontent.com/Vidvox/ISF-Files/master/${p// /%20}" -o "$f" || echo "failed: $p" >&2
  done
ls *.fs | wc -l
