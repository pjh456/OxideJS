use crate::bench::metrics::MetricCollection;

pub struct Baseline {
    pub entries: Vec<MetricCollection>,
}

impl Baseline {
    pub fn empty() -> Self {
        Self { entries: Vec::new() }
    }
}

pub struct Regression {
    pub metric: String,
    pub test_name: String,
    pub baseline: f64,
    pub current: f64,
    pub ratio: f64,
    pub tolerance: f64,
}

pub fn load_baseline() -> Result<Baseline, String> {
    let json_path = "benchmark_baseline.json";
    let data = match std::fs::read_to_string(json_path) {
        Ok(d) => d,
        Err(_) => {
            eprintln!("No baseline found at {} — run --update-baseline first", json_path);
            return Ok(Baseline::empty());
        }
    };
    let entries: Vec<MetricCollection> =
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse baseline: {}", e))?;
    Ok(Baseline { entries })
}

pub fn save_baseline(results: &[MetricCollection]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(results).map_err(|e| format!("Failed to serialize baseline: {}", e))?;
    std::fs::write("benchmark_baseline.json", &json)
        .map_err(|e| format!("Failed to write benchmark_baseline.json: {}", e))?;
    let md = crate::bench::output::format_text_table(results);
    std::fs::write("BENCHMARK_BASELINE.md", md).map_err(|e| format!("Failed to write BENCHMARK_BASELINE.md: {}", e))?;
    Ok(())
}

pub fn compare_baseline(current: &[MetricCollection]) -> Vec<Regression> {
    let baseline = match load_baseline() {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    if baseline.entries.is_empty() {
        eprintln!("No baseline data — skipping regression check");
        return Vec::new();
    }
    let mut regressions = Vec::new();
    for cur in current {
        for base in &baseline.entries {
            if base.test_name != cur.test_name {
                continue;
            }
            for (name, cur_val) in cur.iter_metrics() {
                if cur_val == 0.0 {
                    continue;
                }
                let base_val = base
                    .iter_metrics()
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| *v)
                    .unwrap_or(cur_val);
                if base_val == 0.0 {
                    continue;
                }
                let tolerance = match name {
                    "wall_time_us" => 0.10,
                    "gc_trigger_count" | "gc_bytes_freed" | "gc_objects_scanned" | "gc_collection_us" => 0.50,
                    "session_bytes" => 0.20,
                    _ => 0.20,
                };
                let ratio = cur_val / base_val;
                if ratio > 1.0 + tolerance {
                    regressions.push(Regression {
                        metric: name.to_string(),
                        test_name: cur.test_name.clone(),
                        baseline: base_val,
                        current: cur_val,
                        ratio,
                        tolerance,
                    });
                }
            }
        }
    }
    regressions
}
