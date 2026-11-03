#!/usr/bin/env bash
# Creates an unrun copy of the regional-revenue analysis for the e2e test.
# Usage: prepare.sh <project-dir>
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="$1"
tmp="$(mktemp -d)"
cargo run --quiet -p vda-core --example slice -- "$tmp" > /dev/null
rm -rf "$out" && mkdir -p "$out"
cp "$tmp/analysis.json" "$root/examples/regional-revenue/sales.csv" "$out/"
rm -rf "$tmp"
