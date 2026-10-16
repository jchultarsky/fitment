"""Throwaway prototype of `fitment catalog`'s user-facing semantics (design C, M5).

It checks the rules of design.md against real files: the gate is the real one
(`fitment features`), properties come from `stepq props`. Storage is a plain
sqlite3 file and content-addressed blobs. Not the implementation: no Rust, no
index tokens, no matching.

Usage: python3 -I proto_catalog.py FITMENT_BIN -C DIR COMMAND [ARGS...]
"""
import hashlib, json, os, re, sqlite3, subprocess, sys, unicodedata

SCHEMA_VERSION = 1
ID_RE = re.compile(r'^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$')
STEP_EXT = ('.stp', '.step', '.p21')
MAX_BYTES = 256 * 1024 * 1024
OK, ERROR, USAGE, REFUSED = 0, 1, 2, 3


def printable(text):
    bad = lambda c: unicodedata.category(c) == 'Cc' or c in '  ‎‏' \
        or '‪' <= c <= '‮' or '⁦' <= c <= '⁩'
    return ''.join(c.encode('unicode_escape').decode() if bad(c) else c for c in text)


def note(line):
    print(line, file=sys.stderr)


def default_id(path):
    """CI3: the file stem, each run of characters outside the id set made one `_`."""
    stem = os.path.basename(path)
    for ext in STEP_EXT:
        if stem.lower().endswith(ext):
            stem = stem[: -len(ext)]
    mapped = re.sub(r'[^A-Za-z0-9._+-]+', '_', stem).lstrip('._+-')[:64]
    return mapped or None


def key(i):
    return i.lower()  # ASCII-only ids: lower() is ASCII case folding


class Catalog:
    def __init__(self, root, fitment):
        self.root, self.fitment = root, fitment
        self.db_path = os.path.join(root, 'catalog.sqlite')

    def is_catalog(self):
        return os.path.isfile(self.db_path)

    def open(self):
        self.db = sqlite3.connect(self.db_path, isolation_level=None)
        self.db.execute('PRAGMA journal_mode=WAL')
        self.db.execute('PRAGMA synchronous=FULL')
        v = self.db.execute("SELECT value FROM meta WHERE name='schema_version'").fetchone()
        return int(v[0])

    def init(self):
        os.makedirs(os.path.join(self.root, 'blobs', 'tmp'), exist_ok=True)
        db = sqlite3.connect(self.db_path, isolation_level=None)
        db.executescript('''
          CREATE TABLE meta(name TEXT PRIMARY KEY, value TEXT);
          CREATE TABLE entries(id_key TEXT PRIMARY KEY, id TEXT, revision INT, status TEXT,
             sha256 TEXT UNIQUE, bytes INT, file_name TEXT, product_id TEXT, product_name TEXT,
             extractor_version INT, features TEXT, refusals TEXT, properties TEXT);
          CREATE TABLE history(id_key TEXT, revision INT, sha256 TEXT, generation INT, event TEXT);
          INSERT INTO meta VALUES('schema_version','1'),('generation','0');''')
        db.close()

    # --- blobs: hex names only (HN1); temp, fsync, rename, fsync dir (ST2)
    def blob_path(self, sha):
        return os.path.join(self.root, 'blobs', sha[:2], sha)

    def write_blob(self, sha, data, fault=None):
        path = self.blob_path(sha)
        if os.path.exists(path):
            return
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = os.path.join(self.root, 'blobs', 'tmp', f'{sha}.{os.getpid()}')
        with open(tmp, 'wb') as fh:
            fh.write(data); fh.flush(); os.fsync(fh.fileno())
        if fault == 'before_rename':
            raise SystemExit(99)
        os.rename(tmp, path)
        os.chmod(path, 0o444)
        dfd = os.open(os.path.dirname(path), os.O_RDONLY)
        os.fsync(dfd); os.close(dfd)

    def gate(self, path):
        r = subprocess.run([self.fitment, 'features', path], capture_output=True)
        try:
            doc = json.loads(r.stdout)
        except ValueError:
            return None
        return doc

    def props(self, path):
        r = subprocess.run(['stepq', 'props', '--format', 'json', '--kind', 'user', path],
                           capture_output=True, text=True)
        out = []
        try:
            doc = json.loads(r.stdout)
        except ValueError:
            return out
        for p in doc.get('properties', []):
            if (p.get('subject') or {}).get('entity') != 'PRODUCT_DEFINITION':
                continue  # PR2: the part's own definition only, never a shape aspect
            for v in p.get('values') or []:
                out.append({'name': p.get('name'), 'value': v.get('value'),
                            'measure': v.get('measure'), 'instance': p.get('instance')})
        return out[:256]

    def bump(self):
        g = int(self.db.execute("SELECT value FROM meta WHERE name='generation'").fetchone()[0]) + 1
        self.db.execute("UPDATE meta SET value=? WHERE name='generation'", (str(g),))
        return g

    def find(self, ref):
        if ref.startswith('sha256:'):
            h = ref[7:].lower()
            if len(h) < 12 or not re.fullmatch('[0-9a-f]+', h):
                return None, 'a sha256 reference needs at least 12 hex digits'
            rows = self.db.execute("SELECT * FROM entries WHERE sha256 LIKE ? AND status!='removed'", (h + '%',)).fetchall()
        else:
            rows = self.db.execute("SELECT * FROM entries WHERE id_key=? AND status!='removed'", (key(ref),)).fetchall()
        if len(rows) == 1:
            return rows[0], None
        if not rows:
            return None, f'no part {printable(ref)} in the catalog'
        return None, f'{printable(ref)} names {len(rows)} parts'


COLS = ['id_key', 'id', 'revision', 'status', 'sha256', 'bytes', 'file_name', 'product_id',
        'product_name', 'extractor_version', 'features', 'refusals', 'properties']


def row(r):
    return dict(zip(COLS, r))


def list_inputs(args, root):
    """MF1's rule: a file as given; a directory's STEP files, sorted, not recursively."""
    out = []
    for a in args:
        if os.path.isdir(a):
            for n in sorted(os.listdir(a)):
                p = os.path.join(a, n)
                if not os.path.isdir(p) and n.lower().endswith(STEP_EXT):
                    out.append(p)
        else:
            out.append(a)
    return out


def ingest(cat, path, want_id, gen_holder, replace_row=None):
    """One file: (result, id, sha, detail, code)."""
    real_root = os.path.realpath(cat.root)
    if os.path.realpath(path).startswith(real_root + os.sep):
        return 'error', '-', '-', 'the file is inside the catalog', USAGE
    try:
        with open(path, 'rb') as fh:
            data = fh.read(MAX_BYTES + 1)  # CI8: read once; hash, gate and store this buffer
    except OSError as e:
        return 'error', '-', '-', f'cannot read: {e.strerror}', ERROR
    if len(data) > MAX_BYTES:
        return 'refused', '-', '-', 'larger than --max-mib', REFUSED
    sha = hashlib.sha256(data).hexdigest()
    ident = want_id if want_id else (replace_row['id'] if replace_row else default_id(path))
    if ident is None or not ID_RE.match(ident):
        return 'conflict', printable(ident or '-'), sha[:8], 'no valid id: give --id', USAGE
    same = cat.db.execute("SELECT id, status FROM entries WHERE sha256=?", (sha,)).fetchone()
    existing = cat.db.execute("SELECT * FROM entries WHERE id_key=?", (key(ident),)).fetchone()
    existing = row(existing) if existing else None
    if same and key(same[0]) == key(ident) and same[1] != 'removed':
        return 'unchanged', ident, sha[:8], None, OK
    if same and same[1] != 'removed':
        return 'duplicate', ident, sha[:8], f'the same bytes are {same[0]}: not added', USAGE
    if replace_row is None and existing and existing['status'] != 'removed':
        return 'conflict', ident, sha[:8], f'the id {ident} is taken by another file (give --id)', USAGE
    # The gate runs outside the write lock: it is the slow part.
    tmp = os.path.join(cat.root, 'blobs', 'tmp', f'in.{os.getpid()}.stp')
    with open(tmp, 'wb') as fh:
        fh.write(data)
    doc = cat.gate(tmp)
    props = cat.props(tmp)
    os.remove(tmp)
    if doc is None:
        return 'error', ident, sha[:8], 'the gate gave no document', ERROR
    accepted = doc['status'] == 'accepted'
    if replace_row is not None and not accepted:
        first = doc['refusals'][0]
        return 'refused', ident, sha[:8], f"{first['code']}: {first['message']} (entry unchanged)", REFUSED
    product = (doc.get('part') or {}).get('definition', {}).get('product') or {}
    cat.db.execute('BEGIN IMMEDIATE')
    try:
        cat.write_blob(sha, data)
        g = cat.bump()
        revision = (existing['revision'] + 1) if existing else 1
        old_sha = existing['sha256'] if existing else None
        cat.db.execute('DELETE FROM entries WHERE id_key=?', (key(ident),))
        cat.db.execute('INSERT INTO entries VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)', (
            key(ident), ident, revision, 'indexed' if accepted else 'quarantined', sha, len(data),
            os.path.basename(path), product.get('id'), product.get('name'), doc['extractor_version'],
            json.dumps(doc) if accepted else None, json.dumps(doc['refusals']), json.dumps(props)))
        event = 'replaced' if replace_row is not None else 'added'
        cat.db.execute('INSERT INTO history VALUES(?,?,?,?,?)', (key(ident), revision, sha, g, event))
        cat.db.execute('COMMIT')
    except BaseException:
        cat.db.execute('ROLLBACK')
        raise
    if old_sha and old_sha != sha:
        try:
            os.chmod(cat.blob_path(old_sha), 0o644); os.remove(cat.blob_path(old_sha))
        except OSError:
            pass  # an orphan is harmless; verify reports it
    internal = any(r['code'] == 'internal' for r in doc['refusals'])
    if accepted:
        part = doc['part']
        detail = f"{len(part['holes'])} holes, {len(part['shafts'])} shafts, {len(part['planes'])} planes"
        return ('replaced' if replace_row is not None else 'added'), ident, sha[:8], detail, OK
    first = doc['refusals'][0]
    return 'quarantined', ident, sha[:8], f"refused: {first['code']}: {first['message']}", ERROR if internal else REFUSED


def worst(codes):
    for c in (ERROR, USAGE, REFUSED):
        if c in codes:
            return c
    return OK


def main(argv):
    fitment = argv[0]
    if argv[1] != '-C':
        note('usage: FITMENT -C DIR COMMAND'); return USAGE
    cat = Catalog(argv[2], fitment)
    cmd, rest = argv[3], argv[4:]
    if cmd == 'init':
        if cat.is_catalog():
            note(f'fitment: {printable(cat.root)}: already a catalog: unchanged'); return OK
        if os.path.isdir(cat.root) and os.listdir(cat.root):
            note(f'error: {printable(cat.root)} is not empty and not a catalog'); return USAGE
        cat.init(); note(f'fitment: {printable(cat.root)}: catalog created'); return OK
    if not cat.is_catalog():
        note(f'error: {printable(cat.root)} is not a catalog (fitment catalog init)'); return USAGE
    if cat.open() > SCHEMA_VERSION:
        note('fitment: refused: the catalog was made by a newer fitment'); return REFUSED
    if cmd == 'add':
        want = None
        if rest[:1] == ['--id']:
            want, rest = rest[1], rest[2:]
        inputs = list_inputs(rest, cat.root)
        if want and len(inputs) != 1:
            note('error: --id needs exactly one file'); return USAGE
        codes, tally = [], {}
        # CI4: files whose stem is already a valid id claim their ids first.
        stem_ok = lambda p: ID_RE.match(os.path.basename(p).rsplit('.', 1)[0]) is not None
        inputs = sorted(inputs, key=lambda p: (not stem_ok(p), p))
        for p in inputs:
            res, ident, sha, detail, code = ingest(cat, p, want, None)
            codes.append(code); tally[res] = tally.get(res, 0) + 1
            print(f'{res:<12} {printable(ident):<24} {sha:<8}  {printable(detail or "")[:110]}')
        note(f'fitment: {len(inputs)} files: ' + ', '.join(f'{v} {k}' for k, v in sorted(tally.items())))
        return worst(codes)
    if cmd in ('list', 'quarantine'):
        rows = [row(r) for r in cat.db.execute("SELECT * FROM entries WHERE status!='removed' ORDER BY id_key")]
        if cmd == 'quarantine':
            rows = [r for r in rows if r['status'] == 'quarantined']
        for r in rows:
            status = r['status']
            if status != 'removed' and r['extractor_version'] != current_extractor(fitment):
                status = 'stale'
            print(f"{printable(r['id']):<24} r{r['revision']:<3} {status:<12} {r['sha256'][:8]}  "
                  f"'{printable(r['product_id'] or '')}'  {printable(r['file_name'] or '')}")
            if cmd == 'quarantine':
                for f in json.loads(r['refusals']):
                    print(f"    {f['code']}: {printable(f['message'])[:120]}")
        return OK
    if cmd == 'get':
        r, why = cat.find(rest[0])
        if r is None:
            note(f'error: {why}'); return USAGE
        r = row(r)
        print(json.dumps({k: r[k] for k in COLS if k not in ('features',)}, indent=1)[:1500])
        for h in cat.db.execute('SELECT revision, sha256, generation, event FROM history WHERE id_key=? ORDER BY revision', (r['id_key'],)):
            print(f'  history r{h[0]} {h[1][:12]} generation {h[2]} {h[3]}')
        return OK
    if cmd == 'replace':
        r, why = cat.find(rest[0])
        if r is None:
            note(f'error: {why}'); return USAGE
        res, ident, sha, detail, code = ingest(cat, rest[1], None, None, replace_row=row(r))
        print(f'{res:<12} {printable(ident):<24} {sha:<8}  {printable(detail or "")[:110]}')
        return code
    if cmd == 'remove':
        keys = [key(i) for i in rest]
        found = [cat.db.execute("SELECT * FROM entries WHERE id_key=? AND status!='removed'", (k,)).fetchone() for k in keys]
        if any(f is None for f in found):
            note('error: unknown ids: nothing removed: ' + ', '.join(printable(i) for i, f in zip(rest, found) if f is None))
            return USAGE
        cat.db.execute('BEGIN IMMEDIATE')
        g = cat.bump()
        for f in found:
            f = row(f)
            cat.db.execute("UPDATE entries SET status='removed', sha256=NULL, features=NULL WHERE id_key=?", (f['id_key'],))
            cat.db.execute('INSERT INTO history VALUES(?,?,?,?,?)', (f['id_key'], f['revision'], f['sha256'], g, 'removed'))
        cat.db.execute('COMMIT')
        for f in found:
            f = row(f)
            try:
                os.chmod(cat.blob_path(f['sha256']), 0o644); os.remove(cat.blob_path(f['sha256']))
            except OSError:
                pass
            print(f"removed      {printable(f['id']):<24} {f['sha256'][:8]}")
        return OK
    if cmd == 'verify':
        problems = 0
        live = {}
        for r in cat.db.execute("SELECT id, sha256, bytes FROM entries WHERE status!='removed'"):
            live[r[1]] = r
            p = cat.blob_path(r[1])
            if not os.path.isfile(p):
                print(f'missing  {printable(r[0])}  {r[1][:12]}'); problems += 1
            elif hashlib.sha256(open(p, 'rb').read()).hexdigest() != r[1]:
                print(f'damaged  {printable(r[0])}  {r[1][:12]}'); problems += 1
        for d, _, fs in os.walk(os.path.join(cat.root, 'blobs')):
            for f in fs:
                if os.path.basename(d) == 'tmp' or f not in live:
                    print(f'orphan   {os.path.relpath(os.path.join(d, f), cat.root)}'); problems += 1
        note(f'fitment: {problems} problems')
        return REFUSED if problems else OK
    note(f'error: unknown command {cmd}'); return USAGE


_EXTRACTOR = None


def current_extractor(fitment):
    global _EXTRACTOR
    if _EXTRACTOR is None:
        _EXTRACTOR = int(os.environ.get('PROTO_EXTRACTOR', '0')) or None
    return _EXTRACTOR if _EXTRACTOR else 6


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
