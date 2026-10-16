# M5 catalog: soundness review

Reviewed: `m5/merged/m5-catalog.md`, against the `m4/exit` worktree (HEAD `9aac308`): `matching/{index,shortlist,matcher,document,signature}.rs`, `features/document.rs`, `fit.rs`, `cmd/match_.rs`, `docs/research/m4-match.md`, and the experiments in `m5/A`, `m5/B` and `m5/C`. Only findings at 80/100 confidence or above are listed.

New experiment: `review/probe/ws` is a scratch copy of `fitment-core`. A test at the end of `matching/signature.rs` (`m5_review_probe`) runs it with `cargo test -p fitment-core --lib m5_review_probe`.

## Findings

### F1. High: dedup copies a verdict on an unchecked column (QR6, QR4, VF1)

**The gap.** QR6 groups survivors by the `features.part_digest` column, and one representative's result is copied to every member. Nothing on the match path ties that column to the document it sits beside:

- QR4 checks `doc` against `doc_sha256`, then `from_json`, `is_current` and the status. It never checks `part_digest`.
- Plain `verify` (VF1, IN5, CT12's list of hand edits) does not check it either. Only `--deep` does (VF2).

B's own table (`B/design.md`, section 8) names the failure, "wrong grouping, so a wrong judgment is copied". B's `catalog check` recomputed the digest. The merge dropped that.

**Scenario.** A batch-ingest or reindex bug writes the digest of good part Y into bad part X's row. This is attack 7's class: "a bug attaches one content's document to another's key". A hand edit does the same. X and Y both survive level 1, and Y has the lower key, so Y is the representative. X is then reported as `InterfaceMatch` under its own SHA-256 and exits 4, while X's own document, which passed QR4, would be rejected. The run exits 0 or 4 on a corrupt row, against M5-32.

**Fix.** QR6 computes the group key from the document it has just loaded and checked (`doc.part_digest()`), and keeps the column for diagnostics only. If that measures too slow, bind the column into the row hash instead (F2's fix) and add `part_digest` to VF1, IN5 and CT12's hand-edit list.

### F2. Medium: a stored document is not bound to the content it is stored under (QR4, QR7, GT2)

**The gap.** `doc_sha256` is the SHA-256 of the document bytes alone. The document holds no content SHA-256: `FeaturesDocument` has a header, but no source. `judge_document(doc, Source::new(row_sha, …))` accepts any pairing of the two.

**Scenario.** A document read from file X ends up under key Y. That takes a misaligned batch, a parallel reindex that zips results in completion order, or a copied row. It passes the hash, `from_json`, `is_current` and the status checks, and plain `verify`. Y is then judged by X's features and reported as Y. This is attack 7 again: the match path itself never catches it, and the design relies on ST2 and `--deep` alone. The same holds for `tokens_sha256`.

**Fix.** Hash a preimage that names the row: `doc_sha256 = SHA-256(content_sha256 ‖ extractor_version ‖ status ‖ part_digest ‖ doc)`, and the same for tokens with `signature_version`. QR4's existing check then catches a substituted or moved row at no extra cost. Add a CT12 case that copies one content's features row to another key.

### F3. High: level 1 drops a part that level 2 keeps and verification can pass (IX5, IX7, section 4.17 step 3)

**The gap.** SG5 lets one candidate stack serve every line of a group: two coaxial required lines within W_L, "two coaxial holes of a clevis". Level 2 allows it (`related`: `x != y || same group`), and so does verification. Level 1's line filter respects it too: it asks for one stack, not a count. `pairs_hold` does not:

- for each parallel `other` line, including one in the same group as `first`, it demands a pair token;
- tokens exist only between distinct stacks (`a.feature == b.feature` is skipped).

So a candidate whose single stack serves both lines has no token and is dropped. The lever plays no part, so IX5's per-part lever neither causes nor fixes this. But M4 never used level 1 on a run, and M5 makes it the gate.

**Verified.** The probe builds a socket with two coaxial shaft lines, Ø10 ± 0.01 over spans [0, 5] and [15, 20], and a part with one Ø10 shaft over [−1, 21]. The results:

- the groups are `[0, 0]`, one group;
- level 2 keeps the part (`Ok`, the budget not exhausted);
- level 1 returns `Ok([])`.

**Consequence.** A full catalog run silently omits a part that `fitment match S file.stp` passes, and may exit 5 ("searched, nothing passed"). That is the outcome M5-13 exists to prevent. None of the planned guards would see it:

- CT1, CT2 and CT11 run on the corpus, and no corpus socket has two same-sense coaxial lines in one socket (scanned: none among B's 32 sockets);
- the CT3 proptest varies tokens and windows, not groups;
- `--audit` is hidden (R18).

**Fix.** In `pairs_hold`, skip any `other` line in `first`'s SG5 group at the part's own (bucketed) lever. Bucketing only coarsens groups, so skipping stays sound. Add the probe as a CT3 case in PR 1, and a constructed clevis-like case to CT1 so an end-to-end test exercises it.

### F4. Medium: `get --format step` can hand out bytes other than those judged (section 7.4, CR3)

**(a) No hash check.** Nothing says `get --format step` re-hashes the blob before writing it: its exit 1 covers only "unreadable". A blob edited in place (ST4's 0444 is advisory), or damaged, is written to `-o` and passed off as the matched part. The match never noticed, since it reads only the index. This silently misreports catalog data.

Fix: hash the bytes against the row's SHA-256 before writing, and on a mismatch exit 1 with "damaged" and `verify`.

**(b) Replace.** A run passes `catalog:BRKT` at r1 (SHA X). Someone then replaces BRKT with Y. `get BRKT --format step` now writes Y without a word. X's blob was unlinked by the replace's inline gc, so the bytes that were judged cannot be recovered. The label carries no revision (open question 2).

Fix: make the label `catalog:<id>@r<n>`; it lies outside the digest, so this costs nothing. Accept `ID@rN` as a reference that fails with exit 2 when that revision is not current, and have `get` print the revision and SHA-256 it wrote on stderr.

### F5. Low: rebuilding `judge_step` on `judge_document` drops the STEP bytes from the fit request (section 5, QR7)

`judge` hands `step` to `FitRequest.candidate_step` (`matcher.rs:651`), and the field is "the candidate's STEP bytes, when the run has them". The proposed `judge_document(&self, doc, source)` has no bytes to give, so a `judge_step` rebuilt on it would ask every verifier with `None`. The cost:

- **M5:** none, since only `NoFitVerifier` runs;
- **M7:** every file run's fit request loses its bytes, and with them its fit check, with no error.

Fix: keep a private `judge_document_with(doc, source, step: Option<&[u8]>)`. `judge_step` passes `Some`, and the catalog passes `None` until QR13.

### F6. Low: reusing an existing blob skips the directory sync, so ST9 fails under power loss (ST3, ST9, CC2)

ST3 (A's `design.md`, line 256) says a final name that already exists is re-hashed and, if equal, reused: there is no rename, so its fan-out directory is not "touched" and not synced before the commit. Two ways this breaks:

- **Concurrent ingest (CC2).** Process A renames X into place and has not yet synced the directory. Process B finds X, reuses it and commits first.
- **A killed ingest.** An ingest is killed after its rename but before its directory sync. A later ingest in the same boot reuses the orphan.

In both cases a power loss then leaves a committed row whose file's directory entry was never made durable, which breaks IN1. A's 300 kill -9 rounds cannot show this, since killing a process loses no page cache. Matches are unaffected; `get`, reindex and M7 are not, and the user's bytes may be gone.

Fix: every blob a commit names has its fan-out directory synced before that commit, whether it was renamed or reused.

### F7. Low: M5-32's "never hidden" holds only for survivors (M5-32, IX9)

Postings carry no hash, and tokens are checked only for ids the posting probe returns. Bit rot in a posting's diameter, or a lost posting row, hides a part, and the run exits 5, not 1. IX9 concedes this ("too little, which hides a part"). M5-32 and CM9 claim more.

Fix: correct M5-32's wording to "a corrupt row that is read". The README should advise running `verify` regularly. Optionally, a match could warn when no successful `verify` has been recorded since generation G.

### Note: IX5's power-of-two bucket must never round down

B's prototype buckets with `bucket.max(lever)`; the merged text omits that guard. `2^ceil(log2(x))` is below x for x one ulp above 2^k when k ≥ 5 (checked in Python: 32, 256 and 1024). The bucket would then ask for more seat groups than level 2 does. Fix: state `max(bucket, lever)` in IX5.

## Checked and sound

These were checked and found sound, unless an F-number above says otherwise.

- **Staleness (RX2, RX3).** The rule is evaluated per live entry, inside the snapshot. Every read path filters by this build's (`EXTRACTOR_VERSION`, `SIGNATURE_VERSION`), and QR4 rechecks `is_current` and the column against the JSON. A half-finished reindex leaves the catalog refused. A newer build's rows are never read. `gc --versions` by a newer build only makes an older build refuse (exit 3), never pass.
- **Level 1 against level 2, apart from F3.**
  - **The line filter** does not depend on the lever.
  - **Seat groups** are joined transitively, and W_L rises with the lever (`linear_window = 2σ + 4·lever·σ_ang`). So a larger lever never asks for more planes, and the part's own lever is exactly level 2's.
  - **The pair reach at the part's own lever** still covers verification. The error is at most 2ε plus drift; the drift is at most 3·lever·σ_ang, and 4·lever·σ_ang is available.
  - **Compact pair tokens `{a, b, distance}`** lose nothing that `pairs_hold` reads.
- **Dedup's premise, apart from F1.** `CandidateResult` depends only on the `Part` and the matcher, with `source` copied in (`matcher.rs` 443 to 474), and `product` and `read` both come from the `Part`. In M5, properties never reach a verdict, a flag or the `Part`.
- **M5-4 and CM4.**
  - **Explicit semantics:** `judge_socket` aligns and verifies even after a level-2 drop.
  - **One entry per content:** MF3 and MF5 merge and order by source SHA-256, which ID5 makes unique.
  - **The digest:** it excludes labels. With `NoFitVerifier` the `step` argument changes nothing.
- **Crashes and concurrency.**
  - **`BEGIN IMMEDIATE`** covers every read-then-write decision.
  - **gc and ingest:** gc's unlink under the write lock after a recheck, with ST6's recheck inside ingest's transaction, is correct; A's `race.rs` implements exactly that.
  - **A match** reads features, tokens and entries in one WAL snapshot and opens no blob, so gc, delete and replace cannot mix states (CR7).
- **Quarantine retry.** A file that becomes accepted is printed (`quarantined -> indexed (now searched)`) and counted on stderr. No path promotes one without a reindex the user runs.
- **Exit codes.**
  - **Nothing searchable:** an empty catalog, all entries quarantined, a stale entry or a blocked socket exits 3.
  - **A row that fails QR4** exits 1, over every verdict.
  - **Exit 5 or 4 without a real search** comes only from F1, F2 (0 or 4), F3 and F7 (5).
- **The PR plan.** Each PR refuses rather than passes when versions move. F1 and F3 ship in PR 4 and PR 1 with no test that would catch them; add the fixes and their tests to those PRs.

## Questions for the user

1. **F3:** should the pair filter skip lines in the same SG5 group (the default proposed here), or should a single stack holding both windows count as a token at distance 0? And given R18, should `--audit` stop being hidden, or run on a sample during `verify`?
2. **F1 and F2:** recompute the group key from each loaded document (robust, costs a serialization per survivor), or bind `part_digest` and the content SHA-256 into the hashed preimage (no cost; catches corruption but not a wrong value written at ingest)? Or both?
3. **Open question 2 and F4(b):** take `catalog:<id>@r<n>` as the label, and `ID@rN` as a strict reference for `get`?
4. **F4(b):** after a replace, should the old blob be kept until gc runs with an explicit flag, so that the bytes a past report judged stay retrievable by `sha256:`? M5-6 deletes it today.
5. **F7:** is "a corrupt posting can hide a part (exit 5) until `verify` runs" acceptable for M5 once documented, or should a match warn when the catalog has not been verified since some generation?
