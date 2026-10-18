#!/usr/bin/env bash
# The API's own documents for a fixed set of requests, for CI to compare
# across Linux, macOS and Windows (docs/research/m6-api.md, VT8): the
# in-process server, ids and time fixed, answers health, readiness,
# `fitment.info`, the OpenAPI document and every error a request reaches
# (tests/golden.rs's scenarios); each response must be the same bytes on
# every OS. PR 5 to 7 add the socket, match and catalog documents.
#
#   tools/api-outputs.sh OUT_DIR    (relative to the workspace)
set -euo pipefail
cd "$(dirname "$0")/.."
out=$1
rm -rf "$out"
mkdir -p "$out"
FITMENT_API_OUTPUTS="$out" cargo test -p fitment-server --test golden --locked --quiet -- \
  --exact writes_the_api_outputs
test -f "$out/openapi.json"
