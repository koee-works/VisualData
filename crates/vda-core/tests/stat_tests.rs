//! Statistical tests: both engines give the same statistics, and they match
//! SciPy on the same data (Welch's t-test, one-way ANOVA, chi-square test of
//! independence without continuity correction, Pearson correlation).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::*;
use vda_core::session::Session;
use vda_core::table::{Table, Value};

/// Deterministic data: three groups with different averages and spreads,
/// two related categories, two correlated numbers, and missing values.
fn data() -> String {
    let mut seed: u64 = 42;
    let mut rnd = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = String::from("group,value,channel,plan,x,y,flat\n");
    for i in 0..240 {
        let g = ["A", "B", "C"][i % 3];
        let shift = [0.0, 1.5, 0.4][i % 3];
        let spread = [1.0, 2.5, 0.7][i % 3];
        let value = 10.0 + shift + spread * (rnd() + rnd() + rnd() - 1.5);
        let channel = if rnd() < 0.5 { "web" } else { "store" };
        // The plan depends on the channel, so they are related.
        let plan = if channel == "web" { if rnd() < 0.7 { "pro" } else { "basic" } } else if rnd() < 0.35 { "pro" } else { "basic" };
        let x = rnd() * 100.0;
        let y = 0.3 * x + 20.0 * rnd();
        let missing = i % 17 == 0;
        out.push_str(&format!(
            "{g},{},{channel},{plan},{x},{},5\n",
            if missing { String::new() } else { format!("{value:.4}") },
            if i % 23 == 0 { String::new() } else { format!("{y}") }
        ));
    }
    out
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-stat-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("data.csv"), data()).unwrap();
    fs::write(dir.join("two.csv"), data().lines().enumerate().filter(|(i, l)| *i == 0 || !l.starts_with('C')).map(|(_, l)| format!("{l}\n")).collect::<String>()).unwrap();
    dir
}

fn test_step(id: &str, source: &str, test: TestKind, a: &str, b: &str) -> Step {
    Step { id: id.into(), inputs: vec![source.into()], label: None, kind: StepKind::Test { test, a: a.into(), b: b.into() } }
}

fn steps() -> Vec<Step> {
    vec![
        test_step("means", "two", TestKind::CompareMeans, "value", "group"),
        test_step("groups", SOURCE_ID, TestKind::CompareGroups, "value", "group"),
        test_step("related", SOURCE_ID, TestKind::Independence, "channel", "plan"),
        test_step("corr", SOURCE_ID, TestKind::Correlation, "x", "y"),
    ]
}

fn run(dir: &Path, engine: Box<dyn Engine>) -> Vec<Table> {
    let a = Analysis {
        name: "tests".into(),
        sources: vec![Source::primary("data.csv"), Source::new("two", "two.csv")],
        steps: steps(),
        cells: vec![],
        parameters: vec![],
    };
    let mut s = Session::new(a, dir, engine, None).unwrap();
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");
    steps().iter().map(|st| s.preview(&st.id, 0, 5).unwrap()).collect()
}

fn f(t: &Table, col: &str) -> f64 {
    let i = t.schema.index_of(col).unwrap_or_else(|| panic!("no column {col}"));
    t.rows[0][i].as_f64().unwrap()
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-300)
}

const SCIPY: &str = r#"
import json, sys
import pandas as pd
from scipy import stats
d = pd.read_csv(sys.argv[1])
two = d[d.group != "C"].dropna(subset=["value"])
a, b = (two[two.group == g].value for g in ("A", "B"))
t = stats.ttest_ind(a, b, equal_var=False)
g = d.dropna(subset=["value"])
f = stats.f_oneway(*(g[g.group == k].value for k in sorted(g.group.unique())))
tab = pd.crosstab(d.channel, d.plan)
chi = stats.chi2_contingency(tab, correction=False)
c = d.dropna(subset=["x", "y"])
r = stats.pearsonr(c.x, c.y)
print(json.dumps({"t": t.statistic, "t_df": t.df, "t_p": t.pvalue, "f": f.statistic, "f_p": f.pvalue,
  "chi": chi.statistic, "chi_df": int(chi.dof), "chi_p": chi.pvalue, "r": r.statistic, "r_p": r.pvalue}))
"#;

fn scipy(dir: &Path) -> Option<serde_like::Map> {
    let out = Command::new(vda_core::python::default_interpreter()).args(["-c", SCIPY, &dir.join("data.csv").to_string_lossy()]).output().ok()?;
    if !out.status.success() {
        eprintln!("skipped SciPy comparison: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""));
        return None;
    }
    Some(serde_like::parse(&String::from_utf8_lossy(&out.stdout)))
}

/// A tiny reader for the flat JSON object SciPy prints.
mod serde_like {
    pub type Map = std::collections::HashMap<String, f64>;
    pub fn parse(s: &str) -> Map {
        let j = vda_core::json::Json::parse(s.trim()).unwrap();
        let vda_core::json::Json::Obj(m) = j else { panic!("not an object") };
        m.into_iter().map(|(k, v)| (k, v.as_f64().unwrap())).collect()
    }
}

#[test]
fn both_engines_agree_and_match_scipy() {
    let duck = run(&project("duck"), Box::new(DuckDbEngine::new().unwrap()));
    let refr = run(&project("ref"), Box::new(ReferenceEngine::new()));
    for (d, r) in duck.iter().zip(&refr) {
        assert_eq!(d.schema, r.schema);
        for (x, y) in d.rows[0].iter().zip(&r.rows[0]) {
            match (x, y) {
                (Value::Float(a), Value::Float(b)) => assert!(close(*a, *b, 1e-9), "{a} vs {b}"),
                _ => assert_eq!(x, y),
            }
        }
    }
    let [means, groups, related, corr] = &duck[..] else { unreachable!() };
    assert_eq!(means.rows[0][0], Value::Text("A".into()));
    assert_eq!(means.rows[0][3], Value::Text("B".into()));
    let reading = |t: &Table| t.rows[0].last().unwrap().to_string();
    assert!(reading(means).starts_with("The average of value differs between A and B"), "{}", reading(means));
    assert!(reading(related).starts_with("channel and plan are related"), "{}", reading(related));
    assert!(reading(corr).starts_with("A strong correlation: y rises as x rises") || reading(corr).starts_with("A moderate correlation"), "{}", reading(corr));

    let Some(sp) = scipy(&project("scipy")) else { return };
    let check = |what: &str, ours: f64, theirs: &str, rel: f64| {
        assert!(close(ours, sp[theirs], rel), "{what}: ours {ours}, SciPy {}", sp[theirs]);
    };
    check("t", f(means, "t"), "t", 1e-9);
    check("t df", f(means, "df"), "t_df", 1e-9);
    check("t p", f(means, "p_value"), "t_p", 1e-8);
    check("F", f(groups, "f"), "f", 1e-9);
    check("F p", f(groups, "p_value"), "f_p", 1e-8);
    check("chi-square", f(related, "chi_square"), "chi", 1e-9);
    check("chi-square df", f(related, "df"), "chi_df", 0.0);
    check("chi-square p", f(related, "p_value"), "chi_p", 1e-8);
    check("r", f(corr, "r"), "r", 1e-9);
    check("r p", f(corr, "p_value"), "r_p", 1e-6);
}

#[test]
fn tests_explain_what_they_need() {
    let dir = project("errors");
    let cases = [
        (test_step("s", SOURCE_ID, TestKind::CompareMeans, "value", "group"), "needs exactly two groups, and `group` has 3 (A, B, C)"),
        (test_step("s", SOURCE_ID, TestKind::CompareMeans, "channel", "group"), "the column to compare needs numbers; `channel` is text"),
        (test_step("s", SOURCE_ID, TestKind::Independence, "x", "plan"), "`x` is decimal (group it first with bucket())"),
        (test_step("s", SOURCE_ID, TestKind::Correlation, "x", "flat"), "every value of `flat` is the same"),
        (test_step("s", SOURCE_ID, TestKind::Correlation, "x", "x"), "choose two different columns"),
    ];
    for (step, want) in cases {
        let a = Analysis { name: "e".into(), sources: vec![Source::primary("data.csv")], steps: vec![step], cells: vec![], parameters: vec![] };
        let mut s = Session::new(a, &dir, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
        let msg = match s.run_all() {
            Err(e) => e.to_string(),
            Ok(r) => format!("{:?}", r.steps[0].status),
        };
        assert!(msg.contains(want), "{msg}\nwanted: {want}");
    }
}

#[test]
fn a_test_result_is_a_table_later_steps_can_read() {
    let dir = project("downstream");
    let mut steps = steps();
    steps.push(Step {
        id: "significant".into(),
        inputs: vec!["corr".into()],
        label: None,
        kind: StepKind::Derive { columns: vec![Derived { name: "small_p".into(), formula: "p_value < 0.05".into() }] },
    });
    let a = Analysis { name: "d".into(), sources: vec![Source::primary("data.csv"), Source::new("two", "two.csv")], steps, cells: vec![], parameters: vec![] };
    let mut s = Session::new(a, &dir, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    assert!(s.run_all().unwrap().succeeded());
    let t = s.preview("significant", 0, 1).unwrap();
    assert_eq!(t.rows[0].last().unwrap(), &Value::Bool(true));
    // Its SQL view says where the test's numbers come from.
    let sql = s.pipeline_sql("significant").unwrap();
    assert!(sql.contains("materialized output of test step `corr`"), "{sql}");
}
