#!/usr/bin/env bash
set -euo pipefail
TAG="${1:-qutebrowser.qutebrowser-qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f}"
IMG="jefzda/sweap-images:${TAG}"
if docker image inspect "$IMG" >/dev/null 2>&1; then
  echo "already have $IMG"
  exit 0
fi
for prefix in docker.m.daocloud.io docker.nju.edu.cn dockerproxy.net registry-1.docker.io; do
  echo "== try $prefix =="
  if [[ "$prefix" == "registry-1.docker.io" ]]; then
    if docker pull "$IMG"; then
      echo "ok direct"
      exit 0
    fi
  else
    if docker pull "${prefix}/${IMG}"; then
      docker tag "${prefix}/${IMG}" "$IMG"
      echo "ok via $prefix"
      docker images "$IMG"
      exit 0
    fi
  fi
  sleep 15
done
echo "all mirrors failed"
exit 1
