"""Census of property names in the fixtures, through `stepq props --format csv`.
Usage: python3 -I props_census.py FIXTURES_DIR OUT_DIR"""
import csv, io, os, re, subprocess, sys, collections, json
root, out = sys.argv[1], sys.argv[2]
files = []
for d, _, fs in os.walk(root, followlinks=True):
    for f in fs:
        if f.lower().endswith(('.stp', '.step', '.p21')):
            files.append(os.path.join(d, f))
files.sort()
names = collections.Counter()
by_file = {}
rows_all = []
for f in files:
    r = subprocess.run(['stepq', 'props', '--format', 'csv', f], capture_output=True, text=True)
    rows = list(csv.DictReader(io.StringIO(r.stdout)))
    rel = os.path.relpath(f, root)
    by_file[rel] = len(rows)
    for row in rows:
        key = (row['kind'], row['name'], row['value_name'])
        names[key] += 1
        row['file'] = rel
        rows_all.append(row)
with open(os.path.join(out, 'props_rows.json'), 'w') as fh:
    json.dump(rows_all, fh, indent=0)
print('files', len(files), 'with props', sum(1 for v in by_file.values() if v))
for (k, n, vn), c in sorted(names.items(), key=lambda x: (-x[1], x[0])):
    if n == 'pmi validation property':
        continue
    print(f'{c:6} {k:10} {n!r:50} {vn!r}')
# material / thread search in names or values
pat = re.compile(r'(?i)materi|thread|tap|alloy|steel|alum|sst|density|finish|coat|M\d+(\.\d+)?\s*[xX]|UNC|UNF|grade')
print('--- name/value hits')
for row in rows_all:
    text = ' | '.join([row['name'], row['value_name'], row['value'], row['description']])
    if pat.search(text) and row['name'] != 'pmi validation property':
        print(row['file'], row['instance'], row['kind'], repr(row['name']), repr(row['value_name']), repr(row['value'][:80]), row['unit'])
