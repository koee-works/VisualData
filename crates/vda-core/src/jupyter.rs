//! Export an analysis as a Jupyter notebook that reproduces it with DuckDB
//! and pandas, outside the app.
//!
//! - Visual and SQL steps become the SQL the app runs (parameters filled in),
//!   creating one DuckDB table per step, named by its id.
//! - Python steps run their code with `df`, `dfs` and `params`, reading
//!   inputs as the app does (pandas nullable types, through Parquet).
//! - Check steps print each rule's result; a check that stops raises.
//! - Charts show their last specification (JupyterLab renders Vega-Lite).
//! - Text cells keep document order; the steps a cell shows are computed
//!   just before it, and steps no cell shows come at the end.

use crate::compile::{quote_ident, step_sql_for, CSV_OPTIONS, JSON_OPTIONS};
use crate::json::Json;
use crate::model::{Analysis, Cell, Rule, SourceFormat, Step, StepKind};
use crate::session::{Output, Session};
use crate::table::Schema;
use crate::Error;
use std::collections::{HashMap, HashSet};

/// A Python string literal holding `s`.
fn py(s: &str) -> String {
    Json::str(s).to_string_compact()
}

/// A triple-quoted Python string for SQL, kept readable.
fn py_sql(sql: &str) -> String {
    format!("\"\"\"\n{}\n\"\"\"", sql.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\""))
}

fn lines(text: &str) -> Json {
    let parts: Vec<&str> = text.split_inclusive('\n').collect();
    Json::Arr(parts.into_iter().map(Json::str).collect())
}

fn markdown(text: &str) -> Json {
    Json::obj(vec![("cell_type", Json::str("markdown")), ("metadata", Json::obj(vec![])), ("source", lines(text))])
}

fn code(text: &str) -> Json {
    Json::obj(vec![
        ("cell_type", Json::str("code")),
        ("execution_count", Json::Null),
        ("metadata", Json::obj(vec![])),
        ("outputs", Json::Arr(vec![])),
        ("source", lines(text.trim_end())),
    ])
}

fn title(step: &Step) -> String {
    match &step.label {
        Some(l) => format!("### {l}\n\nStep `{}` ({}), reading {}.", step.id, step.kind.type_name(), reads(step)),
        None => format!("### Step `{}`\n\n{} step, reading {}.", step.id, step.kind.type_name(), reads(step)),
    }
}

fn reads(step: &Step) -> String {
    step.inputs.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ")
}

struct Writer<'a> {
    a: &'a Analysis,
    /// As written, before parameters were filled in (for messages).
    written: &'a Analysis,
    schemas: HashMap<String, Schema>,
    charts: HashMap<String, Json>,
    done: HashSet<String>,
    cells: Vec<Json>,
}

impl Writer<'_> {
    fn ensure(&mut self, id: &str) -> Result<(), Error> {
        if self.done.contains(id) || self.a.is_source(id) {
            return Ok(());
        }
        let step = self.a.step(id).ok_or_else(|| Error::Validation(format!("unknown step `{id}`")))?.clone();
        for input in &step.inputs {
            self.ensure(input)?;
        }
        self.done.insert(id.to_string());
        self.cells.push(markdown(&title(&step)));
        self.cells.push(code(&self.step_code(&step)?));
        Ok(())
    }

    fn input_schemas(&self, step: &Step) -> Result<Vec<Schema>, Error> {
        step.inputs
            .iter()
            .map(|i| {
                self.schemas.get(i).cloned().ok_or_else(|| {
                    Error::Validation(format!("run the analysis before exporting a notebook: `{i}` has no result yet"))
                })
            })
            .collect()
    }

    fn step_code(&self, step: &Step) -> Result<String, Error> {
        let id = &step.id;
        let table = py(&quote_ident(id));
        let input_view = format!("con.sql({})\n", py(&format!("CREATE OR REPLACE TEMP VIEW input AS SELECT * FROM {}", quote_ident(step.input()))));
        let show = format!("con.table({table}).df().head(10)");
        Ok(match &step.kind {
            StepKind::Python { code, .. } => {
                let dfs: Vec<String> = step.inputs.iter().map(|i| format!("{}: frame({})", py(i), py(i))).collect();
                format!(
                    "dfs = {{{}}}\ndf = dfs[{}]\n\n{}\n\nsave({}, result)\n{show}",
                    dfs.join(", "),
                    py(step.input()),
                    code.trim_end(),
                    py(id)
                )
            }
            StepKind::Chart { .. } => match self.charts.get(id) {
                Some(spec) => format!(
                    "# The chart as the app last drew it (Vega-Lite; JupyterLab displays it).\nfrom IPython.display import display\ndisplay({{\"application/vnd.vegalite.v5+json\": json.loads({})}}, raw=True)",
                    py(&spec.to_string_compact())
                ),
                None => "# Run the analysis in the app to include this chart.".into(),
            },
            StepKind::Check { rules, stop } => {
                let schemas = self.input_schemas(step)?;
                let input = &schemas[0];
                let mut out = input_view.clone();
                out.push_str(&format!("con.sql({})\nfailed = []\n", py(&format!("CREATE OR REPLACE TABLE {} AS SELECT * FROM input", quote_ident(id)))));
                let written: Vec<Rule> = match self.written.step(id).map(|s| &s.kind) {
                    Some(StepKind::Check { rules, .. }) => rules.clone(),
                    _ => rules.clone(),
                };
                for (r, w) in rules.iter().zip(&written) {
                    let what = crate::check::describe(w);
                    let (sql, ok) = match r {
                        Rule::Expect { formula } => {
                            let expr = crate::formula::parse(formula).map_err(|m| Error::Validation(format!("step `{id}`: {m}")))?;
                            let cond = crate::formula::to_sql(&expr, input).map_err(|m| Error::Validation(format!("step `{id}`: {m}")))?;
                            (format!("SELECT count(*) FROM input WHERE NOT coalesce(({cond}), false)"), "n == 0".to_string())
                        }
                        Rule::Unique { columns } => {
                            let cols: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
                            (
                                format!("SELECT count(*) FROM (SELECT {} FROM input GROUP BY ALL HAVING count(*) > 1)", cols.join(", ")),
                                "n == 0".to_string(),
                            )
                        }
                        Rule::RowCount { min, max } => {
                            let mut conds = Vec::new();
                            if let Some(m) = min {
                                conds.push(format!("n >= {m}"));
                            }
                            if let Some(m) = max {
                                conds.push(format!("n <= {m}"));
                            }
                            ("SELECT count(*) FROM input".to_string(), conds.join(" and "))
                        }
                    };
                    out.push_str(&format!(
                        "n = con.sql({}).fetchall()[0][0]\nprint(\"passed\" if {ok} else \"FAILED\", {}, \"-\", n)\nif not ({ok}):\n    failed.append({})\n",
                        py(&sql),
                        py(&what),
                        py(&what)
                    ));
                }
                if *stop {
                    out.push_str("assert not failed, \"checks failed: \" + \"; \".join(failed)\n");
                }
                out
            }
            _ => {
                let schemas = self.input_schemas(step)?;
                let sql = step_sql_for(step, &schemas)?.ok_or_else(|| Error::Unsupported(format!("step `{id}` has no SQL form")))?;
                format!(
                    "{input_view}con.sql({} + {})\n{show}",
                    py(&format!("CREATE OR REPLACE TABLE {} AS SELECT * FROM (", quote_ident(id))),
                    py_sql(&format!("{sql}\n)"))
                )
            }
        })
    }
}

/// The notebook (nbformat 4) for the session's analysis. Needs a result
/// (current or not) for every step whose SQL depends on its inputs' columns.
pub fn notebook(session: &mut Session) -> Result<Json, Error> {
    let a = session.analysis.bound();
    let mut schemas = HashMap::new();
    let mut charts = HashMap::new();
    let ids: Vec<String> = a.sources.iter().map(|s| s.id.clone()).chain(a.steps.iter().map(|s| s.id.clone())).collect();
    for id in ids {
        if let Some(s) = session.schema_of(&id) {
            schemas.insert(id.clone(), s);
        }
        if let Some(Output::Chart(spec)) = session.output(&id) {
            charts.insert(id, spec.clone());
        }
    }
    let project = session.project_dir().canonicalize().unwrap_or_else(|_| session.project_dir().to_path_buf());
    let mut setup = format!(
        "# Reproduces \"{}\" from its source files with DuckDB and pandas.\n\
         import json, tempfile\nfrom pathlib import Path\nimport duckdb\nimport pandas as pd\n\n\
         PROJECT = Path({})  # the project folder\nWORK = Path(tempfile.mkdtemp())\ncon = duckdb.connect()\n\n\
         def lit(path):\n    return \"'\" + str(path).replace(\"'\", \"''\") + \"'\"\n\n\
         def frame(name):\n    \"\"\"A table as a pandas DataFrame with nullable types, as Python steps see it.\"\"\"\n    \
         path = WORK / f\"{{name}}.parquet\"\n    con.sql(f'COPY (SELECT * FROM \"{{name}}\") TO {{lit(path)}} (FORMAT parquet)')\n    \
         return pd.read_parquet(path, dtype_backend=\"numpy_nullable\")\n\n\
         def save(name, result):\n    con.register(\"__vda_result\", result)\n    \
         con.sql(f'CREATE OR REPLACE TABLE \"{{name}}\" AS SELECT * FROM __vda_result')\n    con.unregister(\"__vda_result\")\n",
        a.name.replace('"', "'"),
        py(&project.to_string_lossy())
    );
    setup.push_str("\n# Parameters, as Python steps see them; SQL below has their values filled in.\nimport datetime\nparams = {");
    let ps: Vec<String> = a
        .parameters
        .iter()
        .map(|p| {
            let v = match &p.value {
                crate::table::Value::Text(s) => py(s),
                crate::table::Value::Bool(b) => if *b { "True".into() } else { "False".into() },
                crate::table::Value::Date(_) => format!("datetime.date.fromisoformat({})", py(&p.value.to_string())),
                v => v.to_string(),
            };
            format!("{}: {v}", py(&p.name))
        })
        .collect();
    setup.push_str(&ps.join(", "));
    setup.push_str("}\n");

    let mut sources = String::from("# Sources, read as the app reads them.\n");
    for src in &a.sources {
        let path = format!("PROJECT / {}", py(&src.path));
        let path = if std::path::Path::new(&src.path).is_absolute() { format!("Path({})", py(&src.path)) } else { path };
        let table = quote_ident(&src.id);
        let read = match src.format {
            SourceFormat::Csv => format!("read_csv({{lit(path)}}, {CSV_OPTIONS})"),
            SourceFormat::Parquet => "read_parquet({lit(path)})".to_string(),
            SourceFormat::Json => format!("read_json({{lit(path)}}, {JSON_OPTIONS})"),
            SourceFormat::Excel => {
                sources.push_str(&format!(
                    "# Spreadsheet: read with pandas (column types may differ slightly from the app's import).\nsave({}, pd.read_excel({path}, sheet_name={}))\n",
                    py(&src.id),
                    src.sheet.as_deref().map(py).unwrap_or_else(|| "0".into())
                ));
                continue;
            }
        };
        sources.push_str(&format!("path = {path}\ncon.sql(f\"\"\"CREATE OR REPLACE TABLE {table} AS SELECT * FROM {read}\"\"\")\n"));
    }

    let written = session.analysis.clone();
    let mut w = Writer { a: &a, written: &written, schemas, charts, done: HashSet::new(), cells: vec![markdown(&format!("# {}", a.name)), code(&setup), code(&sources)] };
    for cell in &a.cells {
        match cell {
            Cell::Markdown { text } => w.cells.push(markdown(text.trim())),
            Cell::Step { step_id } => w.ensure(step_id)?,
        }
    }
    let rest: Vec<String> = crate::plan::validate(&a)?.into_iter().filter(|id| !w.done.contains(id)).collect();
    if !rest.is_empty() {
        w.cells.push(markdown("## Other steps\n\nSteps the notebook text does not show."));
        for id in rest {
            w.ensure(&id)?;
        }
    }
    Ok(Json::obj(vec![
        ("nbformat", Json::Num(4.0)),
        ("nbformat_minor", Json::Num(5.0)),
        (
            "metadata",
            Json::obj(vec![
                (
                    "kernelspec",
                    Json::obj(vec![("display_name", Json::str("Python 3")), ("language", Json::str("python")), ("name", Json::str("python3"))]),
                ),
                ("language_info", Json::obj(vec![("name", Json::str("python"))])),
            ]),
        ),
        ("cells", Json::Arr(w.cells)),
    ]))
}
