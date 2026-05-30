#!/usr/bin/env bash
set -euo pipefail
GRPCURL_VER=1.9.3
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
curl -fsSL --retry 5 --retry-delay 3 \
  "https://github.com/fullstorydev/grpcurl/releases/download/v${GRPCURL_VER}/grpcurl_${GRPCURL_VER}_linux_x86_64.tar.gz" \
  | tar -xz -C "$TMP"
install -m 755 "$TMP/grpcurl" /usr/local/bin/grpcurl
grpcurl -version
mkdir -p /var/log/uenv /tmp/uenv/wal
echo "install-grpcurl: OK"
