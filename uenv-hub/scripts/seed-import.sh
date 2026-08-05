#!/usr/bin/env bash
# Import a seed SQL dump (produced by seed-export.sh) into a database that has
# already had migrations applied (e.g. by starting the server once) (L10).
#
# Usage: seed-import.sh <db-path> <seed.sql>
set -euo pipefail

DB_PATH="${1:?usage: seed-import.sh <db-path> <seed.sql>}"
SEED="${2:?usage: seed-import.sh <db-path> <seed.sql>}"

sqlite3 "$DB_PATH" < "$SEED"
echo "seed imported into $DB_PATH"
