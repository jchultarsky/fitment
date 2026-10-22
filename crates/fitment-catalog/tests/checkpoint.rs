//! CA3 (`docs/research/m6-api.md`, VT38): the catalog says what a WAL
//! checkpoint did, so the server sees readers pinning the log (CX6)
//! without reading the catalog's files. A reader whose snapshot began
//! before the writes keeps a passive checkpoint from folding the log back
//! and a truncating one busy; once it ends, the log is emptied.

mod common;

use common::{core_tests, scratch};
use fitment_catalog::{Catalog, Options};

#[test]
fn a_reader_pinning_the_wal_is_seen() {
    let dir = scratch("checkpoint");
    let root = dir.join("cat");
    let options = Options::default().with_busy_timeout_ms(100);
    let (mut writer, _) = Catalog::init(&root, options).unwrap();
    let emptied = writer.checkpoint_truncate().unwrap();
    assert!(!emptied.busy);
    assert_eq!(emptied.log_pages, 0);
    let mut reader = Catalog::open(&root, options).unwrap();
    let snapshot = reader.snapshot().unwrap();
    let inputs = [core_tests().join("candidates")];
    writer.ingest(&inputs, None).unwrap();
    let passive = writer.checkpoint_passive().unwrap();
    assert!(passive.log_pages > 0, "{passive:?}");
    assert!(
        passive.checkpointed_pages < passive.log_pages,
        "the reader pins the log: {passive:?}"
    );
    let pinned = writer.checkpoint_truncate().unwrap();
    assert!(pinned.busy, "{pinned:?}");
    assert!(pinned.log_pages > 0, "{pinned:?}");
    drop(snapshot);
    let folded = writer.checkpoint_passive().unwrap();
    assert!(!folded.busy);
    assert_eq!(folded.checkpointed_pages, folded.log_pages, "{folded:?}");
    let emptied = writer.checkpoint_truncate().unwrap();
    assert!(!emptied.busy, "{emptied:?}");
    assert_eq!(emptied.log_pages, 0, "{emptied:?}");
}
