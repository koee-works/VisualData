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

# Optional second project: two sources, a join, and SQL reading two inputs.
# Usage: prepare.sh <project-dir> <join-project-dir>
if [ "${2:-}" != "" ]; then
  join="$2"
  rm -rf "$join" && mkdir -p "$join"
  cp "$root/examples/regional-revenue/sales.csv" "$root/examples/regional-revenue/regions.csv" \
    "$root/crates/vda-core/tests/fixtures/regions.xlsx" "$join/"
  cat > "$join/analysis.json" <<'JSON'
{
  "format_version": 2,
  "name": "Revenue against target",
  "sources": [
    {"id": "source", "path": "sales.csv", "format": "csv"},
    {"id": "regions", "path": "regions.csv", "format": "csv"},
    {"id": "sheet", "path": "regions.xlsx", "format": "excel", "sheet": "Targets"}
  ],
  "steps": [
    {"id": "by_region", "inputs": ["source"], "label": "Revenue by region",
     "op": {"type": "aggregate", "group_by": ["region"], "measures": [
       {"func": "COUNT", "column": null, "alias": "orders"},
       {"func": "SUM", "column": "revenue", "alias": "revenue"}]}},
    {"id": "vs_target", "inputs": ["by_region", "regions"], "label": "Revenue against target",
     "op": {"type": "join", "how": "full", "on": [{"left": "region", "right": "region"}]}},
    {"id": "below", "inputs": ["vs_target", "regions"], "label": "Regions below target",
     "op": {"type": "sql", "sql": "SELECT region, manager, revenue, target\nFROM input\nWHERE region IN (SELECT region FROM regions) AND coalesce(revenue, 0) < target\nORDER BY region"}},
    {"id": "all_regions", "inputs": ["regions", "sheet"], "label": "Regions from both files",
     "op": {"type": "append", "distinct": false, "origin": "from_file"}},
    {"id": "progress", "inputs": ["vs_target"], "label": "Progress against target",
     "op": {"type": "derive", "columns": [
       {"name": "share_of_target", "formula": "round(coalesce(revenue, 0) / target, 3)"},
       {"name": "status", "formula": "if(share_of_target >= 1, \"met\", \"below\")"}]}},
    {"id": "top", "inputs": ["progress"], "label": "Closest to target",
     "op": {"type": "sort", "by": [{"column": "share_of_target", "descending": true}], "limit": 2}},
    {"id": "running", "inputs": ["source"], "label": "Running revenue by region",
     "op": {"type": "window", "partition_by": ["region"], "order_by": [{"column": "order_date", "descending": false}],
       "columns": [{"name": "running_revenue", "func": "running_sum", "column": "revenue", "n": null}]}},
    {"id": "by_product", "inputs": ["source"], "label": "Revenue by region and product",
     "op": {"type": "pivot", "rows": ["region"], "column": "product", "value": "revenue", "func": "SUM"}}
  ],
  "cells": [
    {"type": "markdown", "text": "## Which regions are below target?"},
    {"type": "step", "step": "vs_target"},
    {"type": "step", "step": "below"}
  ]
}
JSON
fi
