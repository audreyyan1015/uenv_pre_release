#!/usr/bin/env bash
set -euo pipefail
# Run on 7143, or another host where the source images are present.
# Registry credentials must be supplied by an authorized administrator.
manifest=${1:-smith-images.registry-manifest.json}
python3 - "$manifest" <<'PY'
import json, subprocess, sys
m=json.load(open(sys.argv[1]))
for row in m['images']:
    src=row['source_image']; dst=row['target_image']
    print(f'PUSH {src} -> {dst}', flush=True)
    subprocess.run(['docker','tag',src,dst], check=True)
    subprocess.run(['docker','push',dst], check=True)
PY
