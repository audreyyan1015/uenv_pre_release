#!/usr/bin/env bash
# Consistent online backup of the UEnvHub SQLite database (L8).
#
# Uses `VACUUM INTO`, which produces a compact, consistent copy while the server
# keeps running (WAL mode). Safe to run from cron.
#
# Usage: backup.sh <db-path> [backup-dir]
set -euo pipefail

DB_PATH="${1:?usage: backup.sh <db-path> [backup-dir]}"
BACKUP_DIR="${2:-./backups}"

mkdir -p "$BACKUP_DIR"
TS="$(date +%Y%m%d-%H%M%S)"
DEST="$BACKUP_DIR/uenv-hub-$TS.db"

# VACUUM INTO requires the destination to not already exist.
sqlite3 "$DB_PATH" "VACUUM INTO '$DEST'"
gzip -f "$DEST"

echo "backup written: ${DEST}.gz"

# Retain only the 14 most recent backups.
ls -1t "$BACKUP_DIR"/uenv-hub-*.db.gz 2>/dev/null | tail -n +15 | xargs -r rm -f
