#!/usr/bin/env bash
# Export environment / version / image / config / template data as a portable
# SQL dump that can be re-imported into a fresh database (L10).
#
# Tokens and the audit log are intentionally excluded.
#
# Usage: seed-export.sh <db-path> [out.sql]
set -euo pipefail

DB_PATH="${1:?usage: seed-export.sh <db-path> [out.sql]}"
OUT="${2:-seed.sql}"

TABLES=(envs env_versions env_images env_configs env_tags env_templates)

{
  echo "-- UEnvHub seed export $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "PRAGMA foreign_keys=OFF;"
  echo "BEGIN TRANSACTION;"
  for t in "${TABLES[@]}"; do
    sqlite3 "$DB_PATH" ".dump $t" | grep -vE '^(PRAGMA|BEGIN|COMMIT|CREATE)'
  done
  echo "COMMIT;"
} > "$OUT"

echo "seed exported to $OUT"
