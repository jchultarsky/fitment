//! CA8 (`docs/research/m6-api.md`, VT38): a catalog's instance id is
//! minted once, by the first caller, and read by every later one. The row
//! is not an `op`: the generation does not rise, schema 1 is unchanged,
//! and `verify` stays clean.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::{catalog, scratch};
use fitment_catalog::{Catalog, Error, Options};

#[test]
fn the_instance_is_minted_once() {
    let dir = scratch("instance");
    let mut catalog = catalog(&dir);
    let root = catalog.root().to_owned();
    let generation = catalog.generation().unwrap();
    let minted = AtomicUsize::new(0);
    let mint = || {
        minted.fetch_add(1, Ordering::Relaxed);
        [0xa5; 16]
    };
    let id = catalog.instance(mint).unwrap();
    assert_eq!(id, "a5".repeat(16));
    assert_eq!(minted.load(Ordering::Relaxed), 1);
    let again = catalog.instance(|| panic!("minted twice")).unwrap();
    assert_eq!(again, id);
    assert_eq!(catalog.generation().unwrap(), generation);
    let ops: i64 = catalog
        .connection()
        .query_row("SELECT count(*) FROM op", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ops, 1, "init's only");
    assert_eq!(catalog.verify(true, true).unwrap(), Vec::new());
    drop(catalog);
    let mut reopened = Catalog::open(&root, Options::default()).unwrap();
    assert_eq!(reopened.instance(|| [0; 16]).unwrap(), id);
    // A value no build writes is damage, never used.
    reopened
        .connection()
        .execute(
            "UPDATE meta SET value = 'NOT-AN-ID' WHERE key = 'instance'",
            [],
        )
        .unwrap();
    assert!(matches!(
        reopened.instance(|| [0; 16]),
        Err(Error::Corrupt(_))
    ));
}

/// Callers at once, each with its own catalog: one id, minted by one.
#[test]
fn callers_at_once_agree_on_one_instance() {
    let dir = scratch("instance-race");
    let root = catalog(&dir).root().to_owned();
    let ids: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8_u8)
            .map(|n| {
                let root = root.clone();
                scope.spawn(move || {
                    let mut catalog = Catalog::open(&root, Options::default()).unwrap();
                    catalog.instance(|| [n; 16]).unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
}
