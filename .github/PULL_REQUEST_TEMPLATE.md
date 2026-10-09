## What

<!-- One or two sentences. Link the issue if there is one. -->

## Why

<!-- The reasoning, for the person reading `git blame` later. -->

## Checklist

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --workspace --all-features --locked`
- [ ] `cargo deny check`, and the `fitment-core` allowlist (see CONTRIBUTING.md)
- [ ] Any new `fitment-core` dependency or `#[allow]` of its bans is justified above
- [ ] Matcher changes: zero false positives on the known-bad corpus; false-negative change noted above
- [ ] New rules have a known-good and a known-bad corpus case
- [ ] Tested against a real STEP file where relevant (say which)
- [ ] `CHANGELOG.md` updated under **Unreleased** if user-visible
- [ ] I agree to license this contribution under the MIT License
