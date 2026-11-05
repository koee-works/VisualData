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
    // Tests run in parallel: each gets its own folder, named for the test.
    let test = std::thread::current().name().unwrap_or("test").replace("::", "-");
    let dir = std::env::temp_dir().join(format!("vda-jupyter-{}-{test}", std::process::id()));
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
        .args(["-c", "import duckdb, pandas, pyarrow, scipy"])
        .status()
        .is_ok_and(|s| s.success())
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
                "rs",
                &["big", "regions"],
                StepKind::R {
                    code: "result <- merge(df, dfs$regions, by = \"region\", all.x = TRUE)\nresult$share <- result$n / sum(result$n)\nresult$limit <- params$min_units".into(),
                    timeout_ms: 60_000,
                },
            ),
            step(
                "quality",
                &["joined"],
                StepKind::Check { stop: false, rules: vec![Rule::Expect { formula: "units >= @min_units".into() }, Rule::Unique { columns: vec!["order_id".into()] }] },
            ),
            step("by_product", &["kept"], StepKind::Test { test: TestKind::CompareGroups, a: "revenue".into(), b: "product".into() }),
            step("related", &["kept"], StepKind::Test { test: TestKind::Independence, a: "region".into(), b: "product".into() }),
            step("moves", &["kept"], StepKind::Test { test: TestKind::Correlation, a: "units".into(), b: "revenue".into() }),
            step(
                "tidy",
                &["source"],
                StepKind::Clean {
                    markers: vec!["N/A".into()],
                    flag: true,
                    seed: 3,
                    internal: false,
                    columns: vec![
                        CleanColumn { missing_when: Some("units > @min_units + 2".into()), ..CleanColumn::new("units", Fill::Regression { predictors: vec!["revenue".into()] }) },
                        CleanColumn::new("region", Fill::Neighbours { predictors: vec!["units".into(), "revenue".into()], k: 3 }),
                        CleanColumn { group_by: vec!["product".into()], ..CleanColumn::new("revenue", Fill::Random) },
                    ],
                },
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
    s.approve_step("rs").unwrap();
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
    let tables = ["kept", "calc", "joined", "running", "latest", "wide", "long", "big", "py", "rs", "quality", "by_product", "related", "moves", "tidy"];
    assert!(script.contains("as the app fitted them"), "fitted fills are written in");
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
            assert!(x.iter().zip(y).all(|(a, b)| same_value(a, b)), "{t} row {i}:\napp: {x:?}\nnb:  {y:?}");
        }
    }
}

/// The steps every notebook language holds: visual, SQL, clean and check steps,
/// the statistical tests and a chart.
fn portable_steps() -> Vec<Step> {
    vec![
        step("kept", &["source"], StepKind::Filter { predicates: vec![Predicate { column: "units".into(), op: CmpOp::Ge, value: Value::Null, param: Some("min_units".into()) }] }),
        step("calc", &["kept"], StepKind::Derive { columns: vec![Derived { name: "adjusted".into(), formula: "revenue * @rate".into() }] }),
        step("flags", &["kept"], StepKind::Derive { columns: vec![Derived { name: "is_pro".into(), formula: "product = 'Pro'".into() }] }),
        step("joined", &["calc", "regions"], StepKind::Join { how: JoinKind::Left, on: vec![JoinKey::equal("region", "region")] }),
        step("latest", &["joined"], StepKind::Dedupe { keys: vec!["region".into()], order_by: vec![SortKey { column: "order_date".into(), descending: true }] }),
        step("big", &["calc"], StepKind::Sql { sql: "SELECT region, count(*) AS n FROM input WHERE units >= $min_units GROUP BY region ORDER BY region".into() }),
        step(
            "quality",
            &["joined"],
            StepKind::Check { stop: false, rules: vec![Rule::Expect { formula: "units >= @min_units".into() }, Rule::Unique { columns: vec!["order_id".into()] }] },
        ),
        step(
            "tidy",
            &["source"],
            StepKind::Clean {
                markers: vec!["N/A".into()],
                flag: true,
                seed: 3,
                internal: false,
                columns: vec![
                    CleanColumn { missing_when: Some("units > @min_units + 2".into()), ..CleanColumn::new("units", Fill::Regression { predictors: vec!["revenue".into()] }) },
                    CleanColumn::new("region", Fill::Neighbours { predictors: vec!["units".into(), "revenue".into()], k: 3 }),
                    CleanColumn { group_by: vec!["product".into()], ..CleanColumn::new("revenue", Fill::Random) },
                ],
            },
        ),
        step("means", &["flags"], StepKind::Test { test: TestKind::CompareMeans, a: "revenue".into(), b: "is_pro".into() }),
        step("by_product", &["kept"], StepKind::Test { test: TestKind::CompareGroups, a: "revenue".into(), b: "product".into() }),
        step("related", &["kept"], StepKind::Test { test: TestKind::Independence, a: "region".into(), b: "product".into() }),
        step("moves", &["kept"], StepKind::Test { test: TestKind::Correlation, a: "units".into(), b: "revenue".into() }),
        step(
            "chart",
            &["kept"],
            StepKind::Chart { mark: Mark::Bar, x: "region".into(), y: "revenue".into(), title: None, color: None, summary: Some(AggFunc::Sum) },
        ),
    ]
}

/// The tables every portable notebook makes (the statistical tests among them).
const PORTABLE_TABLES: [&str; 12] = ["kept", "calc", "flags", "joined", "latest", "big", "quality", "tidy", "means", "by_product", "related", "moves"];

fn portable_parameters() -> Vec<Parameter> {
    vec![Parameter { name: "min_units".into(), value: Value::Int(2) }, Parameter { name: "rate".into(), value: Value::Float(1.25) }]
}

fn portable_session(dir: &Path, steps: Vec<Step>) -> Session {
    let a = Analysis {
        name: "Portable".into(),
        sources: vec![Source::primary("sales.csv"), Source::new("regions", "regions.csv")],
        parameters: portable_parameters(),
        steps,
        cells: vec![],
    };
    Session::new(a, dir, Box::new(DuckDbEngine::new().unwrap()), Some(PythonRunner::new(vda_core::python::default_interpreter(), dir.join(".work")))).unwrap()
}

/// The code cells of a notebook, as text.
fn code_cells(nb: &Json) -> Vec<String> {
    nb.get("cells")
        .and_then(Json::as_arr)
        .unwrap()
        .iter()
        .filter(|c| c.get("cell_type").and_then(Json::as_str) == Some("code"))
        .map(|c| c.get("source").and_then(Json::as_arr).unwrap().iter().filter_map(Json::as_str).collect::<String>())
        .collect()
}

/// Runs the SQL notebook's cells in DuckDB, with the parts of ipython-sql a script
/// can stand in for: `%%sql` cells, `x = %sql ...` (a result with DataFrame()),
/// `%sql --persist name` (a DataFrame to a table), and `%sql ...` lines. The
/// Python cells run as they are; charts are not shown.
const SQL_RUNNER: &str = r#"
import duckdb, json, re, sys
nb = json.load(open(sys.argv[1], encoding="utf-8"))
out = sys.argv[2]
con = duckdb.connect()

class _Result:
    def __init__(self, relation):
        self.relation = relation
    def DataFrame(self):
        return self.relation.df()

def _persist(name):
    con.register("__vda_persist", globals()[name])
    con.execute(f'CREATE OR REPLACE TABLE "{name}" AS SELECT * FROM __vda_persist')
    con.unregister("__vda_persist")

for cell in nb["cells"]:
    if cell["cell_type"] != "code":
        continue
    src = "".join(cell["source"])
    if src.startswith("%%sql\n"):
        body = src[len("%%sql\n"):]
        res = con.execute(body)
        if "UNION ALL" in body:
            print(res.fetchall())
        continue
    lines = []
    for line in src.split("\n"):
        if line.startswith("%load_ext") or line.startswith("%sql duckdb"):
            continue
        m = re.match(r"^(\w+) = %sql (.*)$", line)
        if m:
            lines.append(f"{m.group(1)} = _Result(con.sql({m.group(2)!r}))")
        elif line.startswith("%sql --persist "):
            lines.append(f"_persist({line.split()[-1]!r})")
        elif line.startswith("%sql "):
            lines.append(f"con.execute({line[len('%sql '):]!r})")
        else:
            lines.append(line.replace("from IPython.display import display", "display = lambda *a, **k: None"))
    exec(compile("\n".join(lines), "cell", "exec"), globals())

for t in TABLES:
    con.execute(f"COPY (SELECT * FROM \"{t}\") TO '{out}/{t}.parquet' (FORMAT parquet)")
"#;

#[test]
fn the_sql_notebook_reproduces_the_data_steps_and_tests() {
    if !python_has_duckdb() {
        eprintln!("skipped: the Python interpreter has no duckdb module");
        return;
    }
    let dir = project();
    let mut s = portable_session(&dir, portable_steps());
    assert!(s.run_all().unwrap().succeeded());

    let nb = vda_core::jupyter::notebook_in(&mut s, vda_core::jupyter::Language::Sql).unwrap();
    let cells = code_cells(&nb);
    assert!(cells[0].starts_with("%load_ext sql\n%sql duckdb:///:memory:"));
    assert!(cells.iter().any(|c| c.contains("as the app fitted them")), "fitted fills are written in");
    let all = cells.join("\n");
    assert!(all.contains("CREATE OR REPLACE TEMP VIEW input") && !all.contains("con.sql("), "no Python steps or duckdb Python calls");
    assert!(all.contains("\"units\" >= 2"), "parameters are filled in");
    assert!(all.contains("f_oneway") && all.contains("chi2_contingency") && all.contains("pearsonr") && all.contains("ttest_ind"), "every test uses SciPy");
    assert!(all.contains("from IPython.display import display"), "the chart is displayed");

    let out = dir.join("from-sql-notebook");
    fs::create_dir_all(&out).unwrap();
    let script = dir.join("sql_runner.py");
    fs::write(&script, format!("TABLES = {PORTABLE_TABLES:?}\n") + SQL_RUNNER).unwrap();
    let nb_path = dir.join("analysis.sql.ipynb");
    fs::write(&nb_path, nb.to_string_pretty()).unwrap();
    let ran = Command::new(vda_core::python::default_interpreter()).arg(&script).arg(&nb_path).arg(&out).current_dir(std::env::temp_dir()).output().unwrap();
    assert!(ran.status.success(), "{}\n{}", String::from_utf8_lossy(&ran.stderr), all);
    let printed = String::from_utf8_lossy(&ran.stdout);
    assert!(printed.contains("units >= @min_units") && printed.contains("order_id"), "the check rules are reported: {printed}");

    for t in PORTABLE_TABLES {
        let app: Table = s.table(t).unwrap();
        let nb: Table = vda_core::duck::read_parquet(&out.join(format!("{t}.parquet"))).unwrap();
        assert_eq!(app.schema, nb.schema, "{t}: columns");
        assert_eq!(app.rows.len(), nb.rows.len(), "{t}: rows");
        for (i, (x, y)) in app.rows.iter().zip(&nb.rows).enumerate() {
            assert!(x.iter().zip(y).all(|(a, b)| same_value(a, b)), "{t} row {i}:\napp: {x:?}\nnb:  {y:?}");
        }
    }
}

/// Equal as the app and R give them: numbers to within floating-point noise,
/// everything else exactly.
fn same_value(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    };
    match (a, b) {
        // R writes TRUE/FALSE as text in its CSV; DuckDB reads "false" back as a boolean.
        (Value::Bool(x), Value::Text(y)) | (Value::Text(y), Value::Bool(x)) => y == &x.to_string().to_lowercase(),
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y || (x - y).abs() <= 1e-9 * x.abs().max(y.abs()),
            _ => a == b,
        },
    }
}

#[test]
fn the_r_notebook_runs_its_tests_and_matches_the_app() {
    if !python_has_duckdb() || Command::new("Rscript").arg("--version").output().is_err() {
        eprintln!("skipped: needs Python with duckdb and Rscript");
        return;
    }
    let dir = project();
    let mut s = portable_session(&dir, portable_steps());
    assert!(s.run_all().unwrap().succeeded());
    let nb = vda_core::jupyter::notebook_in(&mut s, vda_core::jupyter::Language::R).unwrap();
    let all = code_cells(&nb).join("\n\n");
    assert!(all.contains("unique(d$g)") && all.contains("t.test(x1, x2)") && all.contains("aov(") && all.contains("chisq.test(") && all.contains("cor.test("));
    assert!(all.contains("IRdisplay::publish_mimebundle"), "the chart is displayed with IRkernel");

    // The R packages DuckDB needs are not installed here, so the notebook runs with
    // its SQL done by Python and its tests done by R: `dbExecute` does nothing, and
    // the pairs each test compares are read from the same SQL in DuckDB.
    let out = dir.join("from-r-notebook");
    fs::create_dir_all(&out).unwrap();
    let body = all
        .replace("library(DBI)\nlibrary(duckdb)\ncon <- dbConnect(duckdb())", "con <- NULL")
        .replace("dbWriteTable(con, name, result, overwrite = TRUE)", "write_out(name, result)")
        .replace("IRdisplay::publish_mimebundle(", "invisible(");
    fs::write(dir.join("notebook.R"), &body).unwrap();
    let out_r = format!("{:?}", out.to_string_lossy());
    let prelude = format!(
        "OUT <- {out_r}\ndbExecute <- function(con, sql) invisible(0)\nwrite_out <- function(name, result) write.csv(result, file.path(OUT, paste0(name, \".csv\")), row.names = FALSE)\n\
         dbGetQuery <- function(con, sql) {{ if (!is.null(PAIRS[[sql]])) PAIRS[[sql]] else data.frame() }}\nsource(\"pairs.R\")\nsource(\"notebook.R\")\n"
    );
    fs::write(dir.join("run.R"), prelude).unwrap();
    // Python runs the notebook's SQL in DuckDB and writes the pairs the tests read.
    let pairs = dir.join("pairs.py");
    fs::write(&pairs, PAIRS_PY).unwrap();
    let made = Command::new(vda_core::python::default_interpreter()).arg(&pairs).arg(&dir).current_dir(std::env::temp_dir()).output().unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let ran = Command::new("Rscript").arg("--no-save").arg("--no-restore").arg("run.R").current_dir(&dir).output().unwrap();
    assert!(ran.status.success(), "{}\n{}", String::from_utf8_lossy(&ran.stderr), body);

    // Python reads R's tables as parquet; the app's own tables are compared with them.
    let conv = dir.join("convert.py");
    fs::write(&conv, CONVERT_PY).unwrap();
    let converted = Command::new(vda_core::python::default_interpreter()).arg(&conv).arg(&out).current_dir(std::env::temp_dir()).output().unwrap();
    assert!(converted.status.success(), "{}", String::from_utf8_lossy(&converted.stderr));
    for t in ["means", "by_product", "related", "moves"] {
        let app: Table = s.table(t).unwrap();
        let r: Table = vda_core::duck::read_parquet(&out.join(format!("{t}.parquet"))).unwrap();
        assert_eq!(app.rows.len(), r.rows.len(), "{t}: rows");
        let names: Vec<&str> = app.schema.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, r.schema.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), "{t}: columns");
        for (i, (x, y)) in app.rows.iter().zip(&r.rows).enumerate() {
            for (c, (a, b)) in x.iter().zip(y).enumerate() {
                assert!(same_value(a, b), "{t} row {i} column {}:\napp: {a:?}\nR:   {b:?}", names[c]);
            }
        }
    }
}

/// Runs the notebook's SQL in DuckDB, in order, and writes the result of every
/// `dbGetQuery` the R code makes (the tests' pairs, the checks' counts, the previews)
/// as CSV, so the R script can read them where it would query DuckDB.
const PAIRS_PY: &str = r#"
import duckdb, re, sys
from pathlib import Path
d = Path(sys.argv[1])
body = (d / "notebook.R").read_text(encoding="utf-8")
con = duckdb.connect()
steps = []
for m in re.finditer(r'dbExecute\(con, r"---\((.*?)\)---"\)', body, re.S):
    steps.append((m.start(), "exec", m.group(1)))
for m in re.finditer(r'dbGetQuery\(con, r"---\((.*?)\)---"\)', body, re.S):
    steps.append((m.start(), "query", m.group(1)))
steps.sort()
lines = ["PAIRS <- list()"]
for _, kind, sql in steps:
    if kind == "exec":
        con.execute(sql)
    else:
        path = d / f"query-{len(lines)}.csv"
        try:
            con.execute(f"COPY ({sql}) TO '{path}' (FORMAT csv, HEADER true)")
        except duckdb.Error:
            continue  # a table R makes later (a preview): R gets an empty result
        lines.append("PAIRS[[" + repr(sql) + "]] <- read.csv('" + str(path) + "', stringsAsFactors = FALSE)")
(d / "pairs.R").write_text("\n".join(lines) + "\n", encoding="utf-8")
"#;

/// Reads R's tables (CSV) and writes each as parquet, for comparison with the app.
const CONVERT_PY: &str = r#"
import duckdb, sys
from pathlib import Path
out = Path(sys.argv[1])
con = duckdb.connect()
for csv in out.glob("*.csv"):
    con.execute(f"COPY (SELECT * FROM read_csv('{csv}', header = true, auto_detect = true)) TO '{csv.with_suffix('.parquet')}' (FORMAT parquet)")
"#;

#[test]
fn the_r_notebook_is_valid_r_and_refuses_what_r_cannot_run() {
    let dir = project();
    let mut steps = portable_steps();
    steps.push(step(
        "rs",
        &["big", "regions"],
        StepKind::R {
            code: "result <- merge(df, dfs$regions, by = \"region\", all.x = TRUE)\nresult$share <- result$n / sum(result$n)\nresult$limit <- params$min_units".into(),
            timeout_ms: 60_000,
        },
    ));
    let mut s = portable_session(&dir, steps);
    s.approve_step("rs").unwrap();
    assert!(s.run_all().unwrap().succeeded());

    let nb = vda_core::jupyter::notebook_in(&mut s, vda_core::jupyter::Language::R).unwrap();
    let cells = code_cells(&nb);
    let all = cells.join("\n");
    assert!(!all.contains("%%sql") && !all.contains("con.sql("), "no SQL magic or Python in the R notebook");
    assert!(all.contains("save(\"rs\", result)") && all.contains("dbExecute(con, r\"---(CREATE OR REPLACE TABLE"));
    // Every code cell is R: the whole notebook parses as one R script.
    let file = dir.join("notebook.R");
    fs::write(&file, &all).unwrap();
    match Command::new("Rscript").args(["-e", "invisible(parse(file = commandArgs(TRUE)[1]))"]).arg(&file).output() {
        Ok(o) => assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr)),
        Err(_) => eprintln!("skipped: R is not installed"),
    }
}

#[test]
fn sql_and_r_notebooks_say_which_python_steps_they_cannot_hold() {
    let dir = project();
    let mut steps = portable_steps();
    steps.push(step("py", &["big"], StepKind::Python { code: "result = df".into(), timeout_ms: 30_000 }));
    let mut s = portable_session(&dir, steps);
    for lang in [vda_core::jupyter::Language::Sql, vda_core::jupyter::Language::R] {
        let err = vda_core::jupyter::notebook_in(&mut s, lang).unwrap_err().to_string();
        assert!(err.contains("`py` (Python step)"), "{err}");
        assert!(!err.contains("`by_product`") && !err.contains("`chart`"), "tests and charts are not listed: {err}");
    }
}
