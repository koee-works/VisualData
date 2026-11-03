//! The exported Jupyter notebook reproduces the analysis outside the app:
//! running its code cells with Python, DuckDB and pandas gives the same
//! table for every step as the app.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use vda_core::duck::DuckDbEngine;
use vda_core::json::Json;
use vda_core::model::*;
use vda_core::params::Parameter;
use vda_core::python::PythonRunner;
use vda_core::session::Session;
use vda_core::table::{Table, Value};

fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-jupyter-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue");
    fs::copy(samples.join("sales.csv"), dir.join("sales.csv")).unwrap();
    fs::copy(samples.join("regions.csv"), dir.join("regions.csv")).unwrap();
    dir
}

fn step(id: &str, inputs: &[&str], kind: StepKind) -> Step {
    Step { id: id.into(), inputs: inputs.iter().map(|s| s.to_string()).collect(), label: Some(format!("The {id} step")), kind }
}

fn python_has_duckdb() -> bool {
    Command::new(vda_core::python::default_interpreter())
        .args(["-c", "import duckdb, pandas, pyarrow"])
        .status()
        .is_ok_and(|s| s.success())
}

fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x == y || (x - y).abs() <= 1e-9 * x.abs().max(y.abs()),
        _ => a == b,
    }
}

#[test]
fn the_notebook_reproduces_every_step() {
    if !python_has_duckdb() {
        eprintln!("skipped: the Python interpreter has no duckdb module");
        return;
    }
    let dir = project();
    let key = |c: &str| SortKey { column: c.into(), descending: false };
    let a = Analysis {
        name: "Revenue \"review\"".into(),
        sources: vec![Source::primary("sales.csv"), Source::new("regions", "regions.csv")],
        parameters: vec![
            Parameter { name: "min_units".into(), value: Value::Int(2) },
            Parameter { name: "rate".into(), value: Value::Float(1.25) },
        ],
        steps: vec![
            step(
                "kept",
                &["source"],
                StepKind::Filter {
                    predicates: vec![Predicate { column: "units".into(), op: CmpOp::Ge, value: Value::Null, param: Some("min_units".into()) }],
                },
            ),
            step("calc", &["kept"], StepKind::Derive { columns: vec![Derived { name: "adjusted".into(), formula: "revenue * @rate".into() }] }),
            step("joined", &["calc", "regions"], StepKind::Join { how: JoinKind::Left, on: vec![JoinKey::equal("region", "region")] }),
            step(
                "running",
                &["joined"],
                StepKind::Window {
                    partition_by: vec!["region".into()],
                    order_by: vec![key("order_date"), key("order_id")],
                    columns: vec![WindowColumn { name: "run".into(), func: WindowFunc::RunningSum, column: Some("adjusted".into()), n: None }],
                },
            ),
            step("latest", &["running"], StepKind::Dedupe { keys: vec!["region".into()], order_by: vec![SortKey { column: "order_date".into(), descending: true }] }),
            step("wide", &["kept"], StepKind::Pivot { rows: vec!["region".into()], column: "product".into(), value: Some("revenue".into()), func: AggFunc::Sum }),
            step("long", &["wide"], StepKind::Unpivot { columns: vec!["Basic".into(), "Pro".into()], name: "product".into(), value: "revenue".into() }),
            step("big", &["calc"], StepKind::Sql { sql: "SELECT region, count(*) AS n FROM input WHERE units >= $min_units GROUP BY region ORDER BY region".into() }),
            step(
                "py",
                &["big", "regions"],
                StepKind::Python {
                    code: "result = df.merge(dfs['regions'], on='region', how='left')\nresult['limit'] = params['min_units']".into(),
                    timeout_ms: 30_000,
                },
            ),
            step(
                "quality",
                &["joined"],
                StepKind::Check { stop: false, rules: vec![Rule::Expect { formula: "units >= @min_units".into() }, Rule::Unique { columns: vec!["order_id".into()] }] },
            ),
            step(
                "chart",
                &["kept"],
                StepKind::Chart { mark: Mark::Bar, x: "region".into(), y: "revenue".into(), title: None, color: None, summary: Some(AggFunc::Sum) },
            ),
        ],
        cells: vec![
            Cell::Markdown { text: "## Running revenue\n\nPer region, in date order.".into() },
            Cell::Step { step_id: "running".into() },
            Cell::Step { step_id: "chart".into() },
        ],
    };
    let mut s = Session::new(a, &dir, Box::new(DuckDbEngine::new().unwrap()), Some(PythonRunner::new(vda_core::python::default_interpreter(), dir.join(".work")))).unwrap();
    s.approve_step("py").unwrap();
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");

    let nb = vda_core::jupyter::notebook(&mut s).unwrap();
    let cells = nb.get("cells").and_then(Json::as_arr).unwrap();
    let text = |c: &Json| c.get("source").and_then(Json::as_arr).unwrap().iter().filter_map(Json::as_str).collect::<String>();
    // Document order: the text, then the steps it shows (with what they need), then the rest.
    let headings: Vec<String> = cells
        .iter()
        .filter(|c| c.get("cell_type").and_then(Json::as_str) == Some("markdown"))
        .map(|c| text(c).lines().next().unwrap_or("").to_string())
        .collect();
    assert_eq!(headings[..3], ["# Revenue \"review\"".to_string(), "## Running revenue".into(), "### The kept step".into()]);
    let pos = |h: &str| headings.iter().position(|x| x == h).unwrap_or_else(|| panic!("{h} in {headings:?}"));
    assert!(pos("### The running step") < pos("### The chart step"));
    assert!(pos("## Other steps") < pos("### The py step"));
    let all: String = cells.iter().map(text).collect();
    assert!(all.contains("(2 /* $min_units */)"), "SQL has the value filled in");
    assert!(all.contains("params = {\"min_units\": 2, \"rate\": 1.25}"));

    // Run the code cells as one script, then save every step's table.
    let out = dir.join("from-notebook");
    fs::create_dir_all(&out).unwrap();
    let mut script: String = cells
        .iter()
        .filter(|c| c.get("cell_type").and_then(Json::as_str) == Some("code"))
        .map(|c| text(c) + "\n\n")
        .collect::<String>()
        .replace("from IPython.display import display\n", "display = lambda *a, **k: None\n");
    let tables = ["kept", "calc", "joined", "running", "latest", "wide", "long", "big", "py", "quality"];
    for t in tables {
        script.push_str(&format!("con.sql(f'COPY (SELECT * FROM \"{t}\") TO {{lit({:?})}} (FORMAT parquet)')\n", out.join(format!("{t}.parquet")).to_string_lossy()));
    }
    fs::write(dir.join("notebook.py"), &script).unwrap();
    fs::write(dir.join("analysis.ipynb"), nb.to_string_pretty()).unwrap();
    let ran = Command::new(vda_core::python::default_interpreter()).arg(dir.join("notebook.py")).current_dir(std::env::temp_dir()).output().unwrap();
    assert!(ran.status.success(), "{}\n{}", String::from_utf8_lossy(&ran.stderr), script);
    let printed = String::from_utf8_lossy(&ran.stdout);
    assert!(printed.contains("passed `units >= @min_units`") && printed.contains("passed one row per order_id"), "{printed}");

    for t in tables {
        let app: Table = s.table(t).unwrap();
        let nb: Table = vda_core::duck::read_parquet(&out.join(format!("{t}.parquet"))).unwrap();
        assert_eq!(app.schema, nb.schema, "{t}: columns");
        assert_eq!(app.rows.len(), nb.rows.len(), "{t}: rows");
        for (i, (x, y)) in app.rows.iter().zip(&nb.rows).enumerate() {
            assert!(x.iter().zip(y).all(|(a, b)| close(a, b)), "{t} row {i}:\napp: {x:?}\nnb:  {y:?}");
        }
    }
}
