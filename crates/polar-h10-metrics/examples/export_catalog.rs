use std::{env, fs, path::PathBuf};

use polar_h10_metrics::{METRIC_CATALOG, MetricDefinition, metric_formula_definition};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserMetric {
    #[serde(flatten)]
    metric: MetricDefinition,
    formula: &'static str,
    formula_template: Option<&'static str>,
    formula_source: &'static str,
}

fn main() {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: export_catalog <output.js>");
    let catalog: Vec<_> = METRIC_CATALOG
        .iter()
        .copied()
        .map(|metric| {
            let formula = metric_formula_definition(metric.id);
            BrowserMetric {
                metric,
                formula: formula.formula,
                formula_template: formula.formula_template,
                formula_source: formula.formula_source,
            }
        })
        .collect();
    let json = serde_json::to_string(&catalog).expect("serialize metric catalog");
    let rendered = format!(
        "// Generated from polar-h10-metrics; do not edit by hand.\nwindow.PolarMetricCatalog = Object.freeze({json});\n"
    );
    fs::write(output, rendered).expect("write browser metric catalog");
}
