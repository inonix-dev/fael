#!/bin/sh
# Fail when a .rs file passes LIMIT lines — past that, split x.rs into x.rs + x/
# (see fael-core/src/query/). Big files already here may shrink, never grow.
set -eu
LIMIT=400
# ponytail: ratchet — path + today's size; delete the line once the file is split
ALLOW='
fael/src/aliases.rs 405
fael/src/hook.rs 1169
fael/src/install.rs 541
fael/src/main.rs 425
fael/tests/hook.rs 442
'
cd "$(dirname "$0")/.."
find fael-core/src fael-core/tests fael/src fael/tests -name '*.rs' -exec wc -l {} + |
  ALLOW="$ALLOW" awk -v limit="$LIMIT" '
    BEGIN { n = split(ENVIRON["ALLOW"], a, "\n"); for (i = 1; i <= n; i++) if (split(a[i], p, " ") == 2) cap[p[1]] = p[2] }
    $2 == "total" { next }
    { max = ($2 in cap) ? cap[$2] : limit }
    $1 > max { printf "%s: %d lines (max %d) — split it, see AGENTS.md\n", $2, $1, max; bad = 1 }
    END { exit bad }'
