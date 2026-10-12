//! The comparison of an assembly read with what its test case's
//! construction says (`tools/gen-corpus.py`, the assembly section;
//! `docs/research/m3-socket.md`, section 13): its parts list, every leaf
//! occurrence's transform, the assembly gate, and the selections to try.
//! Shared by fitment-core's tests (the committed set) and fitment's
//! (the corpus).

use fitment_core::{AssemblyRead, DefinitionKind, Listing, PartsDocument, read_assembly};
use serde_json::Value;

/// The assembly gate checks built so far (`m3-socket.md`, 4.1).
pub const BUILT_CHECK: u64 = 35;

/// How far a transform entry may differ from the construction's: its
/// values are rounded to 9 decimals, and Open CASCADE writes 12 to 15
/// significant digits.
const TRANSFORM_TOL: f64 = 1e-8;

fn product_id(listing: &Listing, definition: usize) -> Option<&str> {
    listing
        .definitions()
        .get(definition)?
        .definition
        .product
        .as_ref()?
        .id
        .as_deref()
}

/// The distinct refusal codes, in order.
fn codes(refusals: &[fitment_core::Refusal]) -> Vec<String> {
    let mut codes: Vec<String> = refusals.iter().map(|r| r.code.name().to_owned()).collect();
    codes.dedup();
    codes
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Every way `bytes` differs from what `case` expects; empty if none.
pub fn check(case: &Value, bytes: &[u8]) -> Vec<String> {
    let read = read_assembly(bytes);
    let expect = &case["expect"];
    let mut failures = Vec::new();
    let listing_expect = &expect["listing"];
    if listing_expect["result"] == "refused" {
        if listing_expect["check"].as_u64().unwrap_or(0) <= BUILT_CHECK {
            let wanted = strings(&listing_expect["refusals"]);
            let found = codes(read.refusals());
            if read.listing().is_some() || found != wanted {
                failures.push(format!("listing: refused {found:?}, expected {wanted:?}"));
            }
        }
        return failures;
    }
    let Some(listing) = read.listing() else {
        return vec![format!("listing: refused {:?}", codes(read.refusals()))];
    };
    failures.extend(parts(listing, listing_expect));
    failures.extend(gate(&read, &expect["assembly_gate"]));
    failures.extend(transforms(listing, &expect["occurrences"]));
    for (k, try_) in expect["select"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(failure) = select(listing, try_) {
            failures.push(format!("select {k}: {failure}"));
        }
    }
    failures.extend(document(bytes));
    failures
}

fn parts(listing: &Listing, expect: &Value) -> Vec<String> {
    let mut failures = Vec::new();
    let form = match listing.path_form() {
        fitment_core::PathForm::UsageId => "usage_id",
        fitment_core::PathForm::Instance => "instance",
    };
    if expect["path_form"] != form {
        failures.push(format!(
            "path form {form}, expected {}",
            expect["path_form"]
        ));
    }
    let mut found: Vec<(String, String, String, usize)> = listing
        .definitions()
        .iter()
        .map(|listed| {
            let product = listed.definition.product.as_ref();
            (
                product.and_then(|p| p.id.clone()).unwrap_or_default(),
                product.and_then(|p| p.name.clone()).unwrap_or_default(),
                match listed.kind {
                    DefinitionKind::Part => "part".to_owned(),
                    DefinitionKind::Assembly => "assembly".to_owned(),
                },
                listed.occurrences.len(),
            )
        })
        .collect();
    let mut wanted: Vec<(String, String, String, usize)> = expect["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            (
                p["id"].as_str().unwrap_or_default().to_owned(),
                p["name"].as_str().unwrap_or_default().to_owned(),
                p["kind"].as_str().unwrap_or_default().to_owned(),
                usize::try_from(p["quantity"].as_u64().unwrap_or(0)).unwrap_or(0),
            )
        })
        .collect();
    found.sort();
    wanted.sort();
    if found != wanted {
        failures.push(format!("parts {found:?}, expected {wanted:?}"));
    }
    let root_ok = listing
        .occurrences()
        .iter()
        .filter(|o| o.path.usages().is_empty())
        .map(|o| o.definition)
        .eq([listing.root()]);
    if !root_ok {
        failures.push("the root is not the one occurrence with the empty path".to_owned());
    }
    failures
}

fn gate(read: &AssemblyRead, expect: &Value) -> Vec<String> {
    let found = codes(read.gate_refusals());
    let wanted = if expect["result"] == "accepted" {
        Vec::new()
    } else if expect["check"].as_u64().unwrap_or(0) > BUILT_CHECK {
        return Vec::new();
    } else {
        strings(&expect["refusals"])
    };
    if found == wanted {
        Vec::new()
    } else {
        vec![format!("assembly gate {found:?}, expected {wanted:?}")]
    }
}

/// Every leaf occurrence's transform, one to one per product, as
/// multisets: the order of occurrences is the file's, not the
/// construction's.
fn transforms(listing: &Listing, expect: &Value) -> Vec<String> {
    let mut failures = Vec::new();
    let Some(wanted) = expect.as_object() else {
        return vec!["no expected occurrences".to_owned()];
    };
    let leaves = listing
        .occurrences()
        .iter()
        .filter(|o| {
            listing
                .definitions()
                .get(o.definition)
                .is_some_and(|d| d.kind == DefinitionKind::Part)
        })
        .count();
    let expected_count: usize = wanted
        .values()
        .filter_map(Value::as_array)
        .map(Vec::len)
        .sum();
    if leaves != expected_count {
        failures.push(format!(
            "{leaves} leaf occurrences, expected {expected_count}"
        ));
    }
    for (product, matrices) in wanted {
        let mut found: Vec<([[f64; 3]; 3], [f64; 3])> = listing
            .occurrences()
            .iter()
            .filter(|o| product_id(listing, o.definition) == Some(product.as_str()))
            .map(|o| (o.transform.rotation(), o.transform.translation()))
            .collect();
        let matrices = matrices.as_array().cloned().unwrap_or_default();
        if found.len() != matrices.len() {
            failures.push(format!(
                "{product}: {} occurrences, expected {}",
                found.len(),
                matrices.len()
            ));
            continue;
        }
        for matrix in matrices {
            let entry = |i: usize, j: usize| matrix[i][j].as_f64().unwrap_or(f64::NAN);
            let close = |(rotation, translation): &([[f64; 3]; 3], [f64; 3])| {
                (0..3).all(|i| {
                    (0..3).all(|j| (rotation[i][j] - entry(i, j)).abs() <= TRANSFORM_TOL)
                        && (translation[i] - entry(i, 3)).abs() <= TRANSFORM_TOL
                })
            };
            match found.iter().position(close) {
                Some(at) => {
                    found.swap_remove(at);
                }
                None => failures.push(format!("{product}: no occurrence at {matrix}")),
            }
        }
    }
    failures
}

/// One selection, against what it must choose or the error it must give.
fn select(listing: &Listing, try_: &Value) -> Option<String> {
    let part = try_["part"].as_str();
    let occurrence = match (&try_["occurrence"], try_["occurrence_of"].as_str()) {
        (Value::String(text), _) => Some(text.clone()),
        (_, Some(product)) => {
            let first = listing
                .occurrences()
                .iter()
                .find(|o| product_id(listing, o.definition) == Some(product))?;
            Some(first.path.display(listing.path_form()))
        }
        _ => None,
    };
    let result = listing.select(part, occurrence.as_deref());
    match (result, try_["error"].as_str()) {
        (Ok(selection), None) => {
            let chosen = product_id(listing, selection.definition);
            let matched_by = serde_json::to_value(selection.matched_by).ok()?;
            let count = u64::try_from(selection.occurrences.len()).ok()?;
            (chosen != try_["result"].as_str()
                || Some(count) != try_["occurrences"].as_u64()
                || matched_by != try_["matched_by"])
                .then(|| {
                    format!(
                        "chose {chosen:?} ({} occurrences, {matched_by}), expected {try_}",
                        selection.occurrences.len()
                    )
                })
        }
        (Ok(selection), Some(error)) => Some(format!(
            "chose {:?}, expected {error}",
            product_id(listing, selection.definition)
        )),
        (Err(found), None) => Some(format!("{}: {found}, expected {try_}", found.code())),
        (Err(found), Some(error)) => {
            if found.code() != error {
                return Some(format!("{}, expected {error}", found.code()));
            }
            let ids: Vec<String> = found
                .candidates()
                .iter()
                .map(|c| {
                    c.definition
                        .product
                        .as_ref()
                        .and_then(|p| p.id.clone())
                        .unwrap_or_default()
                })
                .collect();
            if let Some(wanted) = try_.get("suggestions") {
                if ids != strings(wanted) {
                    return Some(format!("suggested {ids:?}, expected {wanted}"));
                }
            }
            if let Some(n) = try_["candidates"].as_u64() {
                if u64::try_from(ids.len()).ok() != Some(n) {
                    return Some(format!("{} candidates, expected {n}", ids.len()));
                }
            }
            None
        }
    }
}

/// The parts document reads back, and the same bytes give the same text.
fn document(bytes: &[u8]) -> Vec<String> {
    let text = PartsDocument::read(bytes).to_json();
    let mut failures = Vec::new();
    if let Err(error) = PartsDocument::from_json(&text) {
        failures.push(format!("the parts document does not read back: {error}"));
    }
    if PartsDocument::read(bytes).to_json() != text {
        failures.push("the parts document differs between two reads".to_owned());
    }
    failures
}
