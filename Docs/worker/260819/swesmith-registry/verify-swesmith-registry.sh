#!/usr/bin/env bash
set -euo pipefail
manifest=${1:-smith-images.registry-manifest.json}
python3 - "$manifest" <<'PY'
import json, subprocess, sys
m=json.load(open(sys.argv[1])); failed=0
for row in m['images']:
    ref=row['target_image']
    p=subprocess.run(['docker','manifest','inspect',ref], text=True, capture_output=True)
    if p.returncode:
        print(f'FAIL {ref}: {p.stderr.strip()}'); failed += 1; continue
    data=json.loads(p.stdout); manifests=data.get('manifests', [])
    platforms={(x.get('platform') or {}).get('os'), (x.get('platform') or {}).get('architecture')} for x in manifests
    if manifests and ('linux','amd64') not in platforms:
        print(f'FAIL {ref}: missing linux/amd64'); failed += 1
    else:
        print(f'OK   {ref}')
sys.exit(1 if failed else 0)
PY
