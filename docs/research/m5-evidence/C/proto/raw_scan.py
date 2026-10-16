"""Raw scan of STEP records for material and thread evidence.
Usage: python3 -I raw_scan.py FIXTURES_DIR"""
import os, re, sys, collections
root = sys.argv[1]
def records(text):
    out, buf, q = [], [], False
    i = 0
    for ch in text:
        if ch == "'":
            q = not q
        if ch == ';' and not q:
            out.append(''.join(buf).strip()); buf = []
        else:
            buf.append(ch)
    return out
files = sorted(os.path.join(d, f) for d, _, fs in os.walk(root, followlinks=True) for f in fs if f.lower().endswith('.stp'))
ent_counts = collections.Counter()
pat = re.compile(r"(?i)material|thread|\bM\d+(\.\d+)?\s*[xX]\s*\d|UNC|UNF|\bSST\b|steel|alumin|alloy|finish|anodi|plating|tapped|\bTAP\b")
kinds = re.compile(r"^#(\d+)\s*=\s*([A-Z_0-9]+|\()")
for f in files:
    text = open(f, encoding='latin-1').read()
    rel = os.path.relpath(f, root)
    for r in records(text):
        m = kinds.match(r)
        if not m: continue
        ent = m.group(2)
        if ent in ('MATERIAL_DESIGNATION','DESCRIPTIVE_REPRESENTATION_ITEM','PROPERTY_DEFINITION','GENERAL_PROPERTY','THREAD','EXTERNALLY_DEFINED_ITEM','CHARACTERIZED_OBJECT','MAKE_FROM_USAGE_OPTION','PRODUCT_RELATED_PRODUCT_CATEGORY'):
            ent_counts[(rel, ent)] += 1
        if pat.search(r) and not r.startswith('#') is False:
            s = re.sub(r'\s+', ' ', r)
            # skip pure geometry with 'thread' in name? keep everything, truncated
            print(f'{rel} :: {s[:220]}')
print('=== entity counts')
for (rel, ent), c in sorted(ent_counts.items()):
    print(f'{c:6} {ent:40} {rel}')
