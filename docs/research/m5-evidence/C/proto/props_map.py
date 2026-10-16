"""PR3's mapping applied to every fixture's part-level user properties.
Usage: python3 -I props_map.py FIXTURES_DIR"""
import json, os, subprocess, sys
root = sys.argv[1]
manifest = {f['path']: f for f in json.load(open(os.path.join(root, 'manifest.json')))['fixtures']}
# PR3: (case-folded name, value as written or None for any) -> (field, meaning). Empty for positive values.
ROWS = {('materialmissingassignments', 'TRUE'): ('material', 'stated_unassigned')}
NOT_MAPPED = {'density': 'a density is not a material (all three files state 1000.0)'}
names = {}
for path in sorted(manifest):
    full = os.path.join(root, path)
    r = subprocess.run(['stepq', 'props', '--format', 'json', '--kind', 'user', full], capture_output=True, text=True)
    doc = json.loads(r.stdout)
    own = [p for p in doc['properties'] if (p.get('subject') or {}).get('entity') == 'PRODUCT_DEFINITION']
    mapped = []
    for p in own:
        n = (p['name'] or '').strip().casefold()
        names.setdefault(p['name'], set()).add(os.path.basename(path))
        for v in p.get('values') or []:
            row = ROWS.get((n, v.get('value'))) or ROWS.get((n, None))
            if row:
                mapped.append((p['instance'], p['name'], v.get('value'), row))
    gate = manifest[path].get('features')
    if own:
        print(f"{os.path.basename(path):40} gate={gate:9} part-level user properties={len(own):3} mapped={mapped}")
print('--- part-level user property names and the files that state them')
for n in sorted(names, key=lambda s: (s or '').casefold()):
    fs = sorted(names[n])
    print(f'{n!r:34} {len(fs):2}  {", ".join(fs)[:110]}')
