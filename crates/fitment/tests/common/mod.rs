//! What the CLI tests share.

use serde_json::Value;

/// What breaks the face partition: a face in two primary features, or a
/// count of faces that differs from the solid's.
#[allow(clippy::missing_panics_doc)] // test support: a malformed document should fail the test
pub fn partition(part: &Value) -> Option<String> {
    let mut faces: Vec<u64> = Vec::new();
    for plane in part["planes"].as_array().unwrap() {
        faces.extend(
            plane["faces"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["face"].as_u64().unwrap()),
        );
    }
    for key in ["cylinders", "cones", "tori"] {
        for feature in part[key].as_array().unwrap() {
            faces.extend(
                feature["faces"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| f.as_u64().unwrap()),
            );
        }
    }
    faces.extend(
        part["other_faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["face"].as_u64().unwrap()),
    );
    let count = faces.len() as u64;
    faces.sort_unstable();
    faces.dedup();
    let total = part["face_counts"]["total"].as_u64().unwrap();
    let solid = part["solids"][0]["faces"].as_u64().unwrap();
    (faces.len() as u64 != count || count != total || total != solid).then(|| {
        format!(
            "the features hold {count} faces, {} distinct, of {total} ({solid} in the solid)",
            faces.len()
        )
    })
}

/// Why fitment-core cannot read back the document `fitment features`
/// printed, if it cannot: every document it writes must keep the rules
/// `from_json` checks.
pub fn reads_back(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    fitment_core::FeaturesDocument::from_json(&text)
        .err()
        .map(|error| format!("its document does not read back: {error}"))
}
