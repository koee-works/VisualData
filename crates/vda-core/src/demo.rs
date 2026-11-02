//! The first-slice demo analysis (also the acceptance-test fixture):
//! CSV → visual filter → visual aggregate → Python step → chart, with notebook text.

use crate::model::*;
use crate::table::Value;

pub const PYTHON_SHARE: &str = r#"# Share of filtered revenue by region, largest first.
result = df.assign(revenue_share=(df["revenue"] / df["revenue"].sum()).round(4))
result = result.sort_values("revenue", ascending=False).reset_index(drop=True)
"#;

pub fn regional_revenue(source_path: &str) -> Analysis {
    Analysis {
        name: "Regional revenue".into(),
        source: Source { path: source_path.into() },
        steps: vec![
            Step {
                id: "valid_orders".into(),
                input: SOURCE_ID.into(),
                label: Some("Orders with a region and revenue of at least 20".into()),
                kind: StepKind::Filter {
                    predicates: vec![
                        Predicate { column: "region".into(), op: CmpOp::IsNotNull, value: Value::Null },
                        Predicate { column: "revenue".into(), op: CmpOp::Ge, value: Value::Int(20) },
                    ],
                },
            },
            Step {
                id: "by_region".into(),
                input: "valid_orders".into(),
                label: Some("Revenue by region".into()),
                kind: StepKind::Aggregate {
                    group_by: vec!["region".into()],
                    measures: vec![
                        Measure { func: AggFunc::Count, column: None, alias: "orders".into() },
                        Measure { func: AggFunc::Sum, column: Some("revenue".into()), alias: "revenue".into() },
                        Measure { func: AggFunc::Avg, column: Some("unit_price".into()), alias: "avg_unit_price".into() },
                    ],
                },
            },
            Step {
                id: "share".into(),
                input: "by_region".into(),
                label: Some("Revenue share".into()),
                kind: StepKind::Python { code: PYTHON_SHARE.into(), timeout_ms: 30_000 },
            },
            Step {
                id: "share_chart".into(),
                input: "share".into(),
                label: Some("Revenue share by region".into()),
                kind: StepKind::Chart {
                    mark: Mark::Bar,
                    x: "region".into(),
                    y: "revenue_share".into(),
                    title: Some("Share of revenue by region".into()),
                },
            },
        ],
        cells: vec![
            Cell::Markdown {
                text: "## Question\n\nWhich regions contribute most of the revenue from meaningful orders?\n\nOrders with no region or revenue under 20 are excluded first.".into(),
            },
            Cell::Step { step_id: "valid_orders".into() },
            Cell::Step { step_id: "by_region".into() },
            Cell::Markdown {
                text: "The share calculation is custom Python because it needs the column total; it reads `by_region` and returns one row per region.".into(),
            },
            Cell::Step { step_id: "share".into() },
            Cell::Step { step_id: "share_chart".into() },
            Cell::Markdown {
                text: "## Caveats\n\nTwo orders have no region and two have no revenue; they are excluded rather than imputed.".into(),
            },
        ],
    }
}
