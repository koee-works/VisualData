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
use crate::describe;
use crate::json::Json;
use crate::model::{Analysis, Cell, Rule, SourceFormat, Step, StepKind};
use crate::session::{Output, Session};
use crate::table::Schema;
use crate::Error;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The language of the code cells in an exported notebook.
///
/// Python runs every kind of step. SQL and R run the data steps (visual, SQL,
/// clean, check; R also runs R steps) and say which steps they cannot hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Python,
    Sql,
    R,
}

impl Language {
    pub fn from_name(name: &str) -> Option<Language> {
        match name.trim().to_ascii_lowercase().as_str() {
            "python" => Some(Language::Python),
            "sql" => Some(Language::Sql),
            "r" => Some(Language::R),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::Python => "Python",
            Language::Sql => "SQL",
            Language::R => "R",
        }
    }
}

/// Steps and sources a notebook in `lang` cannot hold, described for a message.
fn unsupported(a: &Analysis, lang: Language) -> Vec<String> {
    let mut out = Vec::new();
    if lang == Language::Python {
        return out;
    }
    for step in &a.steps {
        let why = match &step.kind {
            StepKind::Python { .. } => Some("Python step"),
            StepKind::R { .. } if lang == Language::Sql => Some("R step"),
            _ => None,
        };
        if let Some(why) = why {
            out.push(format!("`{}` ({why})", step.id));
        }
    }
    for src in &a.sources {
        if matches!(src.format, SourceFormat::Sqlite | SourceFormat::DuckDb) && src.query.is_some() {
            out.push(format!("source `{}` (database query)", src.id));
        }
    }
    out
}

/// A SQL string literal holding `s`.
fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// An R string literal holding `s`.
fn rstr(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

/// An R raw string holding `s`, so SQL needs no escaping.
fn raw(s: &str) -> String {
    format!("r\"---({s})---\"")
}

/// A cell that runs one SQL statement in the SQL or R notebook; `note` goes above it.
fn run_sql(lang: Language, sql: &str, note: Option<&str>) -> String {
    match lang {
        Language::Sql => format!("%%sql\n{}{}", note.map(|n| format!("-- {n}\n")).unwrap_or_default(), sql.trim().trim_end_matches(';')),
        Language::R => format!("{}dbExecute(con, {})", note.map(|n| format!("# {n}\n")).unwrap_or_default(), raw(sql)),
        Language::Python => format!("con.sql({})", py(sql)),
    }
}

/// A cell that shows the first rows of a step's table.
fn preview(lang: Language, id: &str) -> String {
    let sql = format!("SELECT * FROM {} LIMIT 10", quote_ident(id));
    match lang {
        Language::Sql => format!("%%sql\n{sql}"),
        Language::R => format!("print(dbGetQuery(con, {}))", raw(&sql)),
        Language::Python => format!("con.table({}).df().head(10)", py(&quote_ident(id))),
    }
}

/// A Python string literal holding `s`.
const R_HELPER: &str = r#"
import csv, subprocess

def r_type(t):
    t = t.upper()
    if t == "BOOLEAN": return "BOOLEAN"
    if t in ("TINYINT", "SMALLINT", "INTEGER", "BIGINT", "HUGEINT", "UTINYINT", "USMALLINT", "UINTEGER", "UBIGINT"): return "BIGINT"
    if t in ("FLOAT", "DOUBLE") or t.startswith("DECIMAL"): return "DOUBLE"
    if t == "DATE": return "DATE"
    if t.startswith("TIMESTAMP") and "TIME ZONE" not in t: return "TIMESTAMP"
    return "VARCHAR"

CSV_OUT = "FORMAT csv, HEADER true, NULLSTR '\\N', QUOTE '\"', ESCAPE '\"', FORCE_QUOTE *"
CSV_IN = "header = true, nullstr = '\\N', allow_quoted_nulls = false, quote = '\"', escape = '\"', timestampformat = '%Y-%m-%d %H:%M:%S.%f', auto_detect = false"

def run_r(name, code, inputs):
    """Run an R step: the first input is `df`, all are in `dfs`, and `result` becomes table `name`."""
    d = WORK / f"r-{name}"
    d.mkdir(exist_ok=True)
    for n, table in enumerate(inputs):
        cols = [(c, r_type(t)) for c, t, *_ in con.sql(f'DESCRIBE SELECT * FROM "{table}"').fetchall()]
        q = lambda c: '"' + c.replace('"', '""') + '"'
        sel = ", ".join(f"strftime({q(c)}, '%Y-%m-%d %H:%M:%S.%f') AS {q(c)}" if t == "TIMESTAMP" else q(c) for c, t in cols)
        con.sql(f"COPY (SELECT {sel} FROM {q(table)}) TO {lit(d / f'in-{n}.csv')} ({CSV_OUT})")
        (d / f"in-{n}.types").write_text("".join(t + "\n" for _, t in cols))
    names = ", ".join(json.dumps(i) for i in inputs)
    (d / "params.R").write_text(R_PARAMS + "\ninput_names <- c(" + names + ")\n", encoding="utf-8")
    (d / "step.R").write_text(code, encoding="utf-8")
    (d / "wrapper.R").write_text(R_WRAPPER, encoding="utf-8")
    # Your R profile is read (unlike in the app), so packages installed from RStudio are found.
    p = subprocess.run(["Rscript", "--no-save", "--no-restore", "wrapper.R", "result.csv", "step.R", *[str(n) for n in range(len(inputs))]],
                       cwd=d, capture_output=True, text=True)
    print(p.stdout, end="")
    if p.returncode != 0:
        raise RuntimeError(f"R step {name}: " + (p.stderr.strip().splitlines() or ["R failed"])[-1])
    types = (d / "result.csv.types").read_text().split()
    with open(d / "result.csv", newline="", encoding="utf-8") as f:
        header = next(csv.reader(f))
    spec = ", ".join("'" + h.replace("'", "''") + "': '" + t + "'" for h, t in zip(header, types))
    con.sql(f"CREATE OR REPLACE TABLE \"{name}\" AS SELECT * FROM read_csv({lit(d / 'result.csv')}, columns = {{{spec}}}, {CSV_IN})")
"#;

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

/// The step's name as a heading (its label, or what it does), as the app shows it.
fn title(step: &Step) -> String {
    format!("### {}\n\nStep `{}`, reads {}.", describe::title(step), step.id, reads(step))
}

fn reads(step: &Step) -> String {
    step.inputs.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ")
}

/// One check rule as SQL: how many rows break it, and whether it passes.
struct RuleSql {
    what: String,
    /// A count expression (or, for a row count, the number of rows).
    count: String,
    /// The pass condition, with `count` in it (SQL).
    ok_sql: String,
    /// The pass condition, with `n` holding the count (R).
    ok_r: String,
}

fn rule_sql(id: &str, rule: &Rule, what: String, input: &Schema) -> Result<RuleSql, Error> {
    Ok(match rule {
        Rule::Expect { formula } => {
            let expr = crate::formula::parse(formula).map_err(|m| Error::Validation(format!("step `{id}`: {m}")))?;
            let cond = crate::formula::to_sql(&expr, input).map_err(|m| Error::Validation(format!("step `{id}`: {m}")))?;
            let count = format!("(SELECT count(*) FROM input WHERE NOT coalesce(({cond}), false))");
            RuleSql { what, ok_sql: format!("{count} = 0"), count, ok_r: "n == 0".into() }
        }
        Rule::Unique { columns } => {
            let cols: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
            let count = format!("(SELECT count(*) FROM (SELECT {} FROM input GROUP BY ALL HAVING count(*) > 1))", cols.join(", "));
            RuleSql { what, ok_sql: format!("{count} = 0"), count, ok_r: "n == 0".into() }
        }
        Rule::RowCount { min, max } => {
            let count = "(SELECT count(*) FROM input)".to_string();
            let (mut sql, mut r) = (Vec::new(), Vec::new());
            if let Some(m) = min {
                sql.push(format!("{count} >= {m}"));
                r.push(format!("n >= {m}"));
            }
            if let Some(m) = max {
                sql.push(format!("{count} <= {m}"));
                r.push(format!("n <= {m}"));
            }
            RuleSql {
                what,
                ok_sql: if sql.is_empty() { "true".into() } else { sql.join(" AND ") },
                ok_r: if r.is_empty() { "TRUE".into() } else { r.join(" && ") },
                count,
            }
        }
    })
}

/// The SQL notebook's first cell: ipython-sql on DuckDB, and the Python its tests use.
const SQL_SETUP: &str = r#"%load_ext sql
%sql duckdb:///:memory:
import json, math
import numpy as np
import pandas as pd
from scipy import stats

def p_text(p):
    return "p < 0.001" if p < 0.001 else f"p = {p:.3f}"

def label(v):
    return str(bool(v)).lower() if isinstance(v, (bool, np.bool_)) else str(v)

def table(**cols):
    """One row, typed as the app's result."""
    out = pd.DataFrame({k: [v] for k, v in cols.items()})
    for k, v in cols.items():
        if isinstance(v, bool) or v is None:
            out[k] = out[k].astype("Float64")
        elif isinstance(v, (int, np.integer)):
            out[k] = out[k].astype("Int64")
    return out
"#;

/// The pairs a test compares, as SQL: both columns, nulls dropped, in group order.
fn pairs_sql(a: &str, b: &str) -> String {
    let (qa, qb) = (quote_ident(a), quote_ident(b));
    format!("SELECT {qa} AS x, {qb} AS g FROM input WHERE {qa} IS NOT NULL AND {qb} IS NOT NULL ORDER BY 2")
}

/// A statistical test in the SQL notebook: the data from SQL, the statistics from SciPy.
/// The same statistics as the Python notebook and the app (stat_test.rs).
fn sql_test_cell(id: &str, test: &crate::model::TestKind, input: &str, a: &str, b: &str) -> String {
    use crate::model::TestKind;
    let body = match test {
        TestKind::CompareMeans => {
            r#"groups = [(g, d.x[d.g == g].tolist()) for g in d.g.unique()]
assert len(groups) == 2, f"comparing two averages needs exactly two groups; there are {len(groups)}"
(g1, x1), (g2, x2) = groups
t = stats.ttest_ind(x1, x2, equal_var=False)  # Welch's t-test
m1, m2 = float(np.mean(x1)), float(np.mean(x2))
g1, g2 = label(g1), label(g2)
reading = (f"The average of {A} differs between {g1} and {g2} ({p_text(t.pvalue)})." if t.pvalue < 0.05
           else f"No clear difference in the average of {A} between {g1} and {g2} ({p_text(t.pvalue)}).")
vda_result = table(group_1=g1, n_1=len(x1), mean_1=m1, group_2=g2, n_2=len(x2), mean_2=m2, difference=m1 - m2,
                   t=float(t.statistic), df=float(t.df), p_value=float(t.pvalue), reading=reading)"#
        }
        TestKind::CompareGroups => {
            r#"groups = [d.x[d.g == g].tolist() for g in d.g.unique()]
f = stats.f_oneway(*groups)  # one-way analysis of variance
k, n = len(groups), sum(len(xs) for xs in groups)
reading = (f"The average of {A} differs among the {k} groups of {B} ({p_text(f.pvalue)})." if f.pvalue < 0.05
           else f"No clear difference in the average of {A} among the {k} groups of {B} ({p_text(f.pvalue)}).")
vda_result = table(groups=k, n=n, f=float(f.statistic), df_between=k - 1, df_within=n - k, p_value=float(f.pvalue), reading=reading)"#
        }
        TestKind::Independence => {
            r#"tab = pd.crosstab(d.x, d.g)
chi = stats.chi2_contingency(tab, correction=False)  # chi-square test of independence
n = int(tab.values.sum())
v = (chi.statistic / (n * (min(tab.shape) - 1))) ** 0.5
smallest = float(chi.expected_freq.min())
reading = (f"{A} and {B} are related ({p_text(chi.pvalue)}; strength {v:.2f} on a 0–1 scale)." if chi.pvalue < 0.05
           else f"No clear relation between {A} and {B} ({p_text(chi.pvalue)}).")
if smallest < 5:
    reading += " Some expected counts are below 5, so the p-value is rough."
vda_result = table(n=n, categories_1=tab.shape[0], categories_2=tab.shape[1], chi_square=float(chi.statistic), df=int(chi.dof),
                   p_value=float(chi.pvalue), cramers_v=float(v), smallest_expected=smallest, reading=reading)"#
        }
        TestKind::Correlation => {
            r#"r = stats.pearsonr(d.x.astype(float), d.g.astype(float))  # Pearson correlation
rr, n = float(r.statistic), len(d)
strength = "strong" if abs(rr) >= 0.7 else "moderate" if abs(rr) >= 0.4 else "weak" if abs(rr) >= 0.1 else "very weak"
reading = (f"A {strength} correlation: {B} {'rises' if rr >= 0 else 'falls'} as {A} rises (r = {rr:.2f}, {p_text(r.pvalue)})." if r.pvalue < 0.05
           else f"No clear correlation between {A} and {B} (r = {rr:.2f}, {p_text(r.pvalue)}).")
t = rr * ((n - 2) / (1 - rr * rr)) ** 0.5 if abs(rr) < 1 else None
vda_result = table(n=n, r=rr, t=t, df=n - 2, p_value=float(r.pvalue), reading=reading)"#
        }
    };
    format!(
        "A, B = {}, {}\n%sql CREATE OR REPLACE TEMP VIEW input AS SELECT * FROM {}\npairs = %sql {}\nd = pairs.DataFrame()\n{body}\n%sql --persist vda_result\n%sql CREATE OR REPLACE TABLE {} AS SELECT * FROM vda_result\n%sql DROP TABLE vda_result\n%sql SELECT * FROM {} LIMIT 10",
        py(a),
        py(b),
        quote_ident(input),
        pairs_sql(a, b),
        quote_ident(id),
        quote_ident(id)
    )
}

/// A statistical test in the R notebook: base R's tests on the pairs DuckDB returns.
fn r_test_cell(id: &str, test: &crate::model::TestKind, input: &str, a: &str, b: &str) -> String {
    use crate::model::TestKind;
    let body = match test {
        TestKind::CompareMeans => {
            r##"groups <- unique(d$g)
xs <- lapply(groups, function(v) d$x[d$g == v])
if (length(xs) != 2) stop(paste0("comparing two averages needs exactly two groups; there are ", length(xs)))
x1 <- xs[[1]]; x2 <- xs[[2]]
g1 <- label(groups[1]); g2 <- label(groups[2])
tt <- t.test(x1, x2)  # Welch's t-test
m1 <- mean(x1); m2 <- mean(x2)
reading <- if (tt$p.value < 0.05) sprintf("The average of %s differs between %s and %s (%s).", A, g1, g2, p_text(tt$p.value)) else sprintf("No clear difference in the average of %s between %s and %s (%s).", A, g1, g2, p_text(tt$p.value))
result <- data.frame(group_1 = g1, n_1 = length(x1), mean_1 = m1, group_2 = g2, n_2 = length(x2), mean_2 = m2, difference = m1 - m2,
                     t = unname(tt$statistic), df = unname(tt$parameter), p_value = unname(tt$p.value), reading = reading, stringsAsFactors = FALSE)"##
        }
        TestKind::CompareGroups => {
            r##"groups <- unique(d$g)
k <- length(groups)
n <- nrow(d)
f <- summary(aov(d$x ~ factor(d$g)))[[1]]  # one-way analysis of variance
Fv <- f[["F value"]][1]; p <- f[["Pr(>F)"]][1]
reading <- if (p < 0.05) sprintf("The average of %s differs among the %d groups of %s (%s).", A, k, B, p_text(p)) else sprintf("No clear difference in the average of %s among the %d groups of %s (%s).", A, k, B, p_text(p))
result <- data.frame(groups = k, n = n, f = Fv, df_between = k - 1, df_within = n - k, p_value = p, reading = reading, stringsAsFactors = FALSE)"##
        }
        TestKind::Independence => {
            r##"tab <- table(d$x, d$g)
chi <- suppressWarnings(chisq.test(tab, correct = FALSE))  # chi-square test of independence
n <- sum(tab)
v <- sqrt(unname(chi$statistic) / (n * (min(dim(tab)) - 1)))
smallest <- min(chi$expected)
reading <- if (chi$p.value < 0.05) sprintf("%s and %s are related (%s; strength %.2f on a 0–1 scale).", A, B, p_text(chi$p.value), v) else sprintf("No clear relation between %s and %s (%s).", A, B, p_text(chi$p.value))
if (smallest < 5) reading <- paste(reading, "Some expected counts are below 5, so the p-value is rough.")
result <- data.frame(n = n, categories_1 = nrow(tab), categories_2 = ncol(tab), chi_square = unname(chi$statistic), df = unname(chi$parameter),
                     p_value = chi$p.value, cramers_v = unname(v), smallest_expected = smallest, reading = reading, stringsAsFactors = FALSE)"##
        }
        TestKind::Correlation => {
            r##"xr <- as.numeric(d$x); gr <- as.numeric(d$g)
r <- cor.test(xr, gr)  # Pearson correlation
rr <- unname(r$estimate); n <- length(xr)
strength <- if (abs(rr) >= 0.7) "strong" else if (abs(rr) >= 0.4) "moderate" else if (abs(rr) >= 0.1) "weak" else "very weak"
reading <- if (r$p.value < 0.05) sprintf("A %s correlation: %s %s as %s rises (r = %.2f, %s).", strength, B, if (rr >= 0) "rises" else "falls", A, rr, p_text(r$p.value)) else sprintf("No clear correlation between %s and %s (r = %.2f, %s).", A, B, rr, p_text(r$p.value))
tval <- if (abs(rr) < 1) rr * sqrt((n - 2) / (1 - rr * rr)) else NA_real_
result <- data.frame(n = n, r = rr, t = tval, df = n - 2, p_value = unname(r$p.value), reading = reading, stringsAsFactors = FALSE)"##
        }
    };
    format!(
        "p_text <- function(p) if (p < 0.001) \"p < 0.001\" else sprintf(\"p = %.3f\", p)\nlabel <- function(v) if (is.logical(v)) tolower(as.character(v)) else as.character(v)\nA <- {}\nB <- {}\n{}\nd <- dbGetQuery(con, r\"---({})---\")\n{body}\nsave({}, result)\n{}",
        rstr(a),
        rstr(b),
        run_sql(Language::R, &format!("CREATE OR REPLACE TEMP VIEW input AS SELECT * FROM {}", quote_ident(input)), None),
        pairs_sql(a, b),
        rstr(id),
        preview(Language::R, id)
    )
}

/// The source as SQL statements (SQL and R notebooks), each one cell.
fn source_cells(a: &Analysis, lang: Language, project: &Path) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    for (n, src) in a.sources.iter().enumerate() {
        let file = project.join(&src.path);
        let path = sql_lit(&file.to_string_lossy());
        let table = quote_ident(&src.id);
        let stmts: Vec<String> = match src.format {
            SourceFormat::Csv => vec![format!("CREATE OR REPLACE TABLE {table} AS SELECT * FROM read_csv({path}, {CSV_OPTIONS})")],
            SourceFormat::Parquet => vec![format!("CREATE OR REPLACE TABLE {table} AS SELECT * FROM read_parquet({path})")],
            SourceFormat::Json => vec![format!("CREATE OR REPLACE TABLE {table} AS SELECT * FROM read_json({path}, {JSON_OPTIONS})")],
            SourceFormat::Excel => {
                let sheet = src.sheet.as_deref().map(|s| format!(", sheet = {}", sql_lit(s))).unwrap_or_default();
                vec![format!("CREATE OR REPLACE TABLE {table} AS SELECT * FROM read_xlsx({path}{sheet})")]
            }
            SourceFormat::Sqlite | SourceFormat::DuckDb => {
                let Some(t) = src.sheet.as_deref() else {
                    return Err(Error::Validation(format!("source `{}`: choose its table before exporting a notebook", src.id)));
                };
                let db = format!("vda_source_{n}");
                let mut v = Vec::new();
                if src.format == SourceFormat::Sqlite {
                    v.push("INSTALL sqlite".to_string());
                    v.push("LOAD sqlite".to_string());
                    v.push(format!("ATTACH IF NOT EXISTS {path} AS {db} (TYPE sqlite, READ_ONLY)"));
                } else {
                    v.push(format!("ATTACH IF NOT EXISTS {path} AS {db} (READ_ONLY)"));
                }
                v.push(format!("CREATE OR REPLACE TABLE {table} AS SELECT * FROM {db}.{}", quote_ident(t)));
                v
            }
        };
        out.extend(stmts.iter().map(|s| run_sql(lang, s, None)));
    }
    Ok(out)
}

struct Writer<'a> {
    a: &'a Analysis,
    lang: Language,
    /// As written, before parameters were filled in (for messages).
    written: &'a Analysis,
    schemas: HashMap<String, Schema>,
    charts: HashMap<String, Json>,
    /// Clean steps with their fitted fills worked out.
    resolved: HashMap<String, Step>,
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
        let code_cells = if self.lang == Language::Python { vec![self.step_code(&step)?] } else { self.portable_cells(&step)? };
        self.cells.extend(code_cells.iter().map(|c| code(c)));
        Ok(())
    }

    /// A chart as the app last drew it. SQL notebooks display it from Python (their
    /// kernel); R notebooks display it with IRdisplay.
    fn chart_cell(&self, lang: Language, id: &str) -> String {
        match (self.charts.get(id), lang) {
            (None, _) => "# Run the analysis in the app to include this chart.".into(),
            (Some(spec), Language::R) => format!(
                "# The chart as the app last drew it (Vega-Lite; JupyterLab displays it).\nIRdisplay::publish_mimebundle(list(\"application/vnd.vegalite.v5+json\" = jsonlite::fromJSON(r\"---({})---\", simplifyVector = FALSE)))",
                spec.to_string_compact()
            ),
            (Some(spec), _) => format!(
                "# The chart as the app last drew it (Vega-Lite; JupyterLab displays it).\nfrom IPython.display import display\ndisplay({{\"application/vnd.vegalite.v5+json\": json.loads({})}}, raw=True)",
                py(&spec.to_string_compact())
            ),
        }
    }

    /// The cells for a step in the SQL or R notebook: visual, SQL, clean and check steps, and R steps in R.
    fn portable_cells(&self, step: &Step) -> Result<Vec<String>, Error> {
        let lang = self.lang;
        let id = &step.id;
        let input_view = run_sql(lang, &format!("CREATE OR REPLACE TEMP VIEW input AS SELECT * FROM {}", quote_ident(step.input())), None);
        match &step.kind {
            StepKind::R { code, .. } => {
                let frames: Vec<String> = step.inputs.iter().map(|i| format!("{} = frame({})", rstr(i), rstr(i))).collect();
                Ok(vec![format!(
                    "dfs <- list({})\ndf <- dfs[[{}]]\n\n{}\n\nsave({}, result)\n{}",
                    frames.join(", "),
                    rstr(step.input()),
                    code.trim_end(),
                    rstr(id),
                    preview(lang, id)
                )])
            }
            StepKind::Chart { .. } => Ok(vec![self.chart_cell(lang, id)]),
            StepKind::Test { test, a, b } => Ok(vec![match lang {
                Language::R => r_test_cell(id, test, step.input(), a, b),
                _ => sql_test_cell(id, test, step.input(), a, b),
            }]),
            StepKind::Check { rules, stop } => {
                let schemas = self.input_schemas(step)?;
                let written: Vec<Rule> = match self.written.step(id).map(|s| &s.kind) {
                    Some(StepKind::Check { rules, .. }) => rules.clone(),
                    _ => rules.clone(),
                };
                let mut sums = Vec::new();
                for (r, w) in rules.iter().zip(&written) {
                    sums.push(rule_sql(id, r, crate::check::describe(w), &schemas[0])?);
                }
                let mut cells = vec![input_view, run_sql(lang, &format!("CREATE OR REPLACE TABLE {} AS SELECT * FROM input", quote_ident(id)), None)];
                match lang {
                    Language::R => {
                        let mut body = String::from("failed <- character(0)\n");
                        for s in &sums {
                            body.push_str(&format!(
                                "n <- dbGetQuery(con, {})$n[1]\ncat(if ({}) \"passed\" else \"FAILED\", {}, \"-\", n, \"\\n\")\nif (!({})) failed <- c(failed, {})\n",
                                raw(&format!("SELECT {} AS n", s.count)),
                                s.ok_r,
                                rstr(&s.what),
                                s.ok_r,
                                rstr(&s.what)
                            ));
                        }
                        if *stop {
                            body.push_str("if (length(failed) > 0) stop(\"checks failed: \", paste(failed, collapse = \"; \"))\n");
                        }
                        cells.push(body);
                    }
                    Language::Sql if !sums.is_empty() => {
                        let union: Vec<String> = sums.iter().map(|s| format!("SELECT {} AS rule, {} AS n, {} AS passed", sql_lit(&s.what), s.count, s.ok_sql)).collect();
                        cells.push(format!(
                            "%%sql\n-- Each rule: the rows that break it (n) and whether it passes. A check that stops a run cannot stop this notebook.\n{}",
                            union.join("\nUNION ALL\n")
                        ));
                    }
                    _ => {}
                }
                cells.push(preview(lang, id));
                Ok(cells)
            }
            _ => {
                let schemas = self.input_schemas(step)?;
                let (shown, note) = match self.resolved.get(id) {
                    Some(r) => (r, Some("The regression's coefficients and the neighbours' values are as the app fitted them to this step's input.")),
                    None => (step, None),
                };
                let sql = step_sql_for(shown, &schemas)?.ok_or_else(|| Error::Unsupported(format!("step `{id}` has no SQL form")))?;
                Ok(vec![
                    input_view,
                    run_sql(lang, &format!("CREATE OR REPLACE TABLE {} AS SELECT * FROM (\n{sql}\n)", quote_ident(id)), note),
                    preview(lang, id),
                ])
            }
        }
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
            StepKind::R { code, .. } => {
                let inputs: Vec<String> = step.inputs.iter().map(|i| py(i)).collect();
                format!("run_r({}, {}, [{}])\n{show}", py(id), py(code), inputs.join(", "))
            }
            StepKind::Chart { .. } => match self.charts.get(id) {
                Some(spec) => format!(
                    "# The chart as the app last drew it (Vega-Lite; JupyterLab displays it).\nfrom IPython.display import display\ndisplay({{\"application/vnd.vegalite.v5+json\": json.loads({})}}, raw=True)",
                    py(&spec.to_string_compact())
                ),
                None => "# Run the analysis in the app to include this chart.".into(),
            },
            StepKind::Test { test, a, b } => {
                // SciPy, as an analyst would check it; the app computes the same
                // statistics from engine summaries (stat_test.rs).
                let (qa, qb) = (quote_ident(a), quote_ident(b));
                let pairs = py(&format!("SELECT {qa} AS x, {qb} AS g FROM input WHERE {qa} IS NOT NULL AND {qb} IS NOT NULL"));
                let groups = py(&format!(
                    "SELECT {qb} AS g, list({qa}) AS xs FROM input WHERE {qa} IS NOT NULL AND {qb} IS NOT NULL GROUP BY 1 ORDER BY 1 ASC NULLS LAST"
                ));
                let (pa, pb) = (py(a), py(b));
                let body = match test {
                    crate::model::TestKind::CompareMeans => format!(
                        "groups = con.sql({groups}).fetchall()\n\
                         assert len(groups) == 2, f\"comparing two averages needs exactly two groups; there are {{len(groups)}}\"\n\
                         (g1, x1), (g2, x2) = groups\n\
                         t = stats.ttest_ind(x1, x2, equal_var=False)  # Welch's t-test\n\
                         m1, m2 = float(np.mean(x1)), float(np.mean(x2))\n\
                         g1, g2 = label(g1), label(g2)\n\
                         reading = (f\"The average of {{A}} differs between {{g1}} and {{g2}} ({{p_text(t.pvalue)}}).\" if t.pvalue < 0.05\n\
                                    else f\"No clear difference in the average of {{A}} between {{g1}} and {{g2}} ({{p_text(t.pvalue)}}).\")\n\
                         result = table(group_1=g1, n_1=len(x1), mean_1=m1, group_2=g2, n_2=len(x2), mean_2=m2, difference=m1 - m2,\n\
                                        t=float(t.statistic), df=float(t.df), p_value=float(t.pvalue), reading=reading)"
                    ),
                    crate::model::TestKind::CompareGroups => format!(
                        "groups = con.sql({groups}).fetchall()\n\
                         f = stats.f_oneway(*[xs for _, xs in groups])  # one-way analysis of variance\n\
                         k, n = len(groups), sum(len(xs) for _, xs in groups)\n\
                         reading = (f\"The average of {{A}} differs among the {{k}} groups of {{B}} ({{p_text(f.pvalue)}}).\" if f.pvalue < 0.05\n\
                                    else f\"No clear difference in the average of {{A}} among the {{k}} groups of {{B}} ({{p_text(f.pvalue)}}).\")\n\
                         result = table(groups=k, n=n, f=float(f.statistic), df_between=k - 1, df_within=n - k, p_value=float(f.pvalue), reading=reading)"
                    ),
                    crate::model::TestKind::Independence => format!(
                        "d = con.sql({pairs}).df()\n\
                         tab = pd.crosstab(d.x, d.g)\n\
                         chi = stats.chi2_contingency(tab, correction=False)  # chi-square test of independence\n\
                         n = int(tab.values.sum())\n\
                         v = (chi.statistic / (n * (min(tab.shape) - 1))) ** 0.5\n\
                         smallest = float(chi.expected_freq.min())\n\
                         reading = (f\"{{A}} and {{B}} are related ({{p_text(chi.pvalue)}}; strength {{v:.2f}} on a 0–1 scale).\" if chi.pvalue < 0.05\n\
                                    else f\"No clear relation between {{A}} and {{B}} ({{p_text(chi.pvalue)}}).\")\n\
                         if smallest < 5:\n    reading += \" Some expected counts are below 5, so the p-value is rough.\"\n\
                         result = table(n=n, categories_1=tab.shape[0], categories_2=tab.shape[1], chi_square=float(chi.statistic), df=int(chi.dof),\n\
                                        p_value=float(chi.pvalue), cramers_v=float(v), smallest_expected=smallest, reading=reading)"
                    ),
                    crate::model::TestKind::Correlation => format!(
                        "d = con.sql({pairs}).df()\n\
                         r = stats.pearsonr(d.x.astype(float), d.g.astype(float))  # Pearson correlation\n\
                         rr, n = float(r.statistic), len(d)\n\
                         strength = \"strong\" if abs(rr) >= 0.7 else \"moderate\" if abs(rr) >= 0.4 else \"weak\" if abs(rr) >= 0.1 else \"very weak\"\n\
                         reading = (f\"A {{strength}} correlation: {{B}} {{'rises' if rr >= 0 else 'falls'}} as {{A}} rises (r = {{rr:.2f}}, {{p_text(r.pvalue)}}).\" if r.pvalue < 0.05\n\
                                    else f\"No clear correlation between {{A}} and {{B}} (r = {{rr:.2f}}, {{p_text(r.pvalue)}}).\")\n\
                         t = rr * ((n - 2) / (1 - rr * rr)) ** 0.5 if abs(rr) < 1 else None\n\
                         result = table(n=n, r=rr, t=t, df=n - 2, p_value=float(r.pvalue), reading=reading)"
                    ),
                };
                format!(
                    "{input_view}import numpy as np\nfrom scipy import stats\nA, B = {pa}, {pb}\n\n\
                     def p_text(p):\n    return \"p < 0.001\" if p < 0.001 else f\"p = {{p:.3f}}\"\n\n\
                     def label(v):\n    return str(bool(v)).lower() if isinstance(v, (bool, np.bool_)) else str(v)\n\n\
                     def table(**cols):\n    \"\"\"One row, typed as the app's result.\"\"\"\n    \
                     out = pd.DataFrame({{k: [v] for k, v in cols.items()}})\n    \
                     for k, v in cols.items():\n        \
                     if isinstance(v, bool) or v is None:\n            out[k] = out[k].astype(\"Float64\")\n        \
                     elif isinstance(v, (int, np.integer)):\n            out[k] = out[k].astype(\"Int64\")\n    \
                     return out\n\n{body}\nsave({}, result)\n{show}",
                    py(id)
                )
            }
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
                let (step, note) = match self.resolved.get(id) {
                    Some(r) => (r, "# The regression's coefficients and the neighbours' values are as the app fitted them to this step's input.\n"),
                    None => (step, ""),
                };
                let sql = step_sql_for(step, &schemas)?.ok_or_else(|| Error::Unsupported(format!("step `{id}` has no SQL form")))?;
                format!(
                    "{note}{input_view}con.sql({} + {})\n{show}",
                    py(&format!("CREATE OR REPLACE TABLE {} AS SELECT * FROM (", quote_ident(id))),
                    py_sql(&format!("{sql}\n)"))
                )
            }
        })
    }
}

/// The notebook (nbformat 4) in Python: the app's default export.
pub fn notebook(session: &mut Session) -> Result<Json, Error> {
    notebook_in(session, Language::Python)
}

/// The notebook (nbformat 4) for the session's analysis, with its code in `lang`.
/// Needs a result (current or not) for every step whose SQL depends on its inputs' columns.
pub fn notebook_in(session: &mut Session, lang: Language) -> Result<Json, Error> {
    let a = session.analysis.bound();
    let missing = unsupported(&a, lang);
    if !missing.is_empty() {
        return Err(Error::Validation(format!(
            "the {} notebook can't include {}. Export the Python notebook to keep them, or change them to visual or SQL steps.",
            lang.label(),
            missing.join(", ")
        )));
    }
    let mut schemas = HashMap::new();
    let mut charts = HashMap::new();
    let ids: Vec<String> = a.sources.iter().map(|s| s.id.clone()).chain(a.steps.iter().map(|s| s.id.clone())).collect();
    let mut resolved = HashMap::new();
    for step in &a.steps {
        if let StepKind::Clean { columns, .. } = &step.kind {
            if columns.iter().any(|c| c.fill.is_fitted()) {
                resolved.insert(step.id.clone(), session.resolved_clean(&step.id)?);
            }
        }
    }
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
    if a.steps.iter().any(|s| matches!(s.kind, StepKind::R { .. })) {
        setup.push_str(&format!(
            "\n# R steps run with Rscript exactly as in the app: the same wrapper, data passed as CSV with its types.\nR_WRAPPER = {}\nR_PARAMS = {}\n{R_HELPER}",
            py(&crate::rlang::wrapper()),
            py(&crate::rlang::params_r(&a.parameters))
        ));
    }

    let mut sources = String::from("# Sources, read as the app reads them.\n");
    for src in &a.sources {
        let path = format!("PROJECT / {}", py(&src.path));
        let path = if std::path::Path::new(&src.path).is_absolute() { format!("Path({})", py(&src.path)) } else { path };
        let table = quote_ident(&src.id);
        let read = match src.format {
            SourceFormat::Csv => format!("read_csv({{lit(path)}}, {CSV_OPTIONS})"),
            SourceFormat::Parquet => "read_parquet({lit(path)})".to_string(),
            SourceFormat::Json => format!("read_json({{lit(path)}}, {JSON_OPTIONS})"),
            SourceFormat::Sqlite | SourceFormat::DuckDb => {
                let sql = match (&src.query, &src.sheet) {
                    (Some(q), _) => q.trim().trim_end_matches(';').to_string(),
                    (None, Some(t)) => format!("SELECT * FROM {}", quote_ident(t)),
                    (None, None) => {
                        return Err(Error::Validation(format!("source `{}`: choose its table before exporting a notebook", src.id)));
                    }
                };
                let open = if src.format == SourceFormat::Sqlite {
                    format!("import sqlite3\nsave({}, pd.read_sql_query({}, sqlite3.connect(f\"file:{{{path}}}?mode=ro\", uri=True)))\n", py(&src.id), py(&sql))
                } else {
                    format!("db = duckdb.connect(str({path}), read_only=True)\nsave({}, db.sql({}).df())\ndb.close()\n", py(&src.id), py(&sql))
                };
                sources.push_str(&format!("# Database: the {} the app reads (read-only).\n{open}", if src.query.is_some() { "query" } else { "table" }));
                continue;
            }
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

    // Python keeps its setup and sources in two cells; SQL and R write one statement per cell.
    let mut opening = vec![markdown(&format!("# {}", a.name))];
    match lang {
        Language::Python => {
            opening.push(code(&setup));
            opening.push(code(&sources));
        }
        Language::Sql => {
            opening.push(markdown(
                "This notebook runs its code as SQL cells with ipython-sql and DuckDB, and its statistical tests and charts in Python: `pip install ipython-sql duckdb-engine scipy pandas`. SQL computes the data; SciPy computes the p-values, which DuckDB has no functions for.",
            ));
            opening.push(code(SQL_SETUP));
            opening.extend(source_cells(&a, lang, &project)?.iter().map(|c| code(c)));
        }
        Language::R => {
            opening.push(markdown("This notebook runs its code as R with DBI and duckdb, and its charts with IRkernel: `install.packages(c(\"DBI\", \"duckdb\"))`."));
            let setup = format!(
                "library(DBI)\nlibrary(duckdb)\ncon <- dbConnect(duckdb())\n\nframe <- function(name) dbReadTable(con, name)\nsave <- function(name, result) dbWriteTable(con, name, result, overwrite = TRUE)\n\n{}",
                crate::rlang::params_r(&a.parameters)
            );
            opening.push(code(&setup));
            opening.extend(source_cells(&a, lang, &project)?.iter().map(|c| code(c)));
        }
    }
    let written = session.analysis.clone();
    let mut w = Writer { a: &a, lang, written: &written, schemas, charts, resolved, done: HashSet::new(), cells: opening };
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
    // SQL cells run on the Python kernel through ipython-sql; R cells need the R kernel (IRkernel).
    let (kernel, language) = match lang {
        Language::Python | Language::Sql => (("Python 3", "python", "python3"), "python"),
        Language::R => (("R", "R", "ir"), "R"),
    };
    Ok(Json::obj(vec![
        ("nbformat", Json::Num(4.0)),
        ("nbformat_minor", Json::Num(5.0)),
        (
            "metadata",
            Json::obj(vec![
                (
                    "kernelspec",
                    Json::obj(vec![("display_name", Json::str(kernel.0)), ("language", Json::str(kernel.1)), ("name", Json::str(kernel.2))]),
                ),
                ("language_info", Json::obj(vec![("name", Json::str(language))])),
            ]),
        ),
        ("cells", Json::Arr(w.cells)),
    ]))
}
