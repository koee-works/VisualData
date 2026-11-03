"""Builds the spreadsheet, Parquet and JSON fixtures used by tests/source_formats.rs.

Run from this folder with openpyxl and pyarrow installed: python3 make_fixtures.py
"""
import datetime as dt
import json

import openpyxl
import pyarrow as pa
import pyarrow.parquet as pq

wb = openpyxl.Workbook()
ws = wb.active
ws.title = "Notes"
ws.append(["This workbook's data is on the Targets sheet."])
t = wb.create_sheet("Targets")
t.append(["region", "manager", "target", "share", "since", "updated", "active", "region", None])
t.append(["East", "Priya Raman", 2400, 0.3, dt.date(2025, 1, 1), dt.datetime(2026, 7, 1, 9, 30), True, "e", "x"])
t.append(["North", "Tom Okafor", 1800, 0.25, dt.date(2025, 3, 1), dt.datetime(2026, 7, 2, 0, 0), False, "n", None])
t.append([None] * 9)  # blank row: skipped
t.append(["South", "Lena Hart", 1500, None, dt.date(2025, 6, 15), dt.datetime(2026, 7, 3, 17, 45, 10), True, "s", None])
t.append(["West", "Diego Alvarez", "=1/0", 0.2, None, None, None, "w", None])  # a formula with no cached value
t.append(["Central", "Mei Chen", 1200, 0.1, dt.date(2026, 1, 1), dt.datetime(2026, 7, 4, 8, 0), True, "c", None])
wb.save("regions.xlsx")

pq.write_table(
    pa.table({
        "region": ["East", "North", "South", "West", None],
        "target": pa.array([2400, 1800, 1500, 2000, 1200], pa.int32()),
        "ratio": pa.array([1.5, 2.25, None, 0.5, 1.0], pa.float32()),
        "since": pa.array([dt.date(2025, 1, 1)] * 5),
    }),
    "targets.parquet",
)

events = [("East", "visit", {"by": "Priya"}), ("West", "call", None), ("North", "visit", {"by": "Tom", "tags": ["a", "b"]})]
with open("events.jsonl", "w") as f:
    for i, (region, kind, extra) in enumerate(events):
        f.write(json.dumps({"id": i + 1, "region": region, "kind": kind, "at": f"2026-07-0{i + 1} 10:00:00", "detail": extra}) + "\n")
with open("events_array.json", "w") as f:
    json.dump([{"id": 1, "region": "East", "ok": True}, {"id": 2, "region": None, "ok": False}], f)
