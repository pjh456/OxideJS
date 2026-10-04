use crate::bench::metrics::MetricCollection;

/// 基准结果集合，持久化到 `benchmark_baseline.json` 用于后续回归对比。
pub struct Baseline {
    pub entries: Vec<MetricCollection>,
}

impl Baseline {
    /// 构造空的基线。
    pub fn empty() -> Self {
        Self { entries: Vec::new() }
    }
}

/// 一次性能回退信号：当前指标相对基线的比值超过容差即报出。
///
/// `informational` 标记信息性信号（墙时型列）：随表格打印供人工参考，
/// 不影响退出码；非信息性信号是回归门，出现即退出码 1。
pub struct Regression {
    pub metric: String,
    pub test_name: String,
    pub baseline: f64,
    pub current: f64,
    pub ratio: f64,
    pub tolerance: f64,
    pub informational: bool,
}

/// 从 `benchmark_baseline.json` 加载基线；文件不存在时返回空基线。
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

/// 保存当前结果到 `benchmark_baseline.json` 并生成 Markdown 表格。
pub fn save_baseline(results: &[MetricCollection]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(results).map_err(|e| format!("Failed to serialize baseline: {}", e))?;
    std::fs::write("benchmark_baseline.json", &json)
        .map_err(|e| format!("Failed to write benchmark_baseline.json: {}", e))?;
    let md = crate::bench::output::format_text_table(results);
    std::fs::write("BENCHMARK_BASELINE.md", md).map_err(|e| format!("Failed to write BENCHMARK_BASELINE.md: {}", e))?;
    Ok(())
}

/// 对比当前结果与基线，返回所有超过各自容差的信号项。
///
/// 列语义分三档：确定性列（指令数、内存账目、垃圾回收计数、内联缓存计数）
/// 跨运行逐字节一致，零容差，双向任何变化即信号；墙时型列（wall、exec、
/// compile、gc_collection）容差 0.50，信息性，只打印不进退出码；
/// `ic_hit_rate` 越高越好（下降超容差才报），其余列越低越好（上升超容差报）。
/// 两侧零值跳过（基线为 0、当前非零的新分配场景由重锚吸收，不在此报）。
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
                // 确定性列零容差（双向任何变化即信号）；墙时型列信息性；
                // ic_hit_rate 越高越好；其余列保留 20% 默认带。
                let (tolerance, informational) = match name {
                    "instruction_count" | "session_objects" | "session_bytes" | "gc_trigger_count"
                    | "gc_bytes_freed" | "gc_objects_scanned" | "ic_hits" | "ic_misses" | "peak_bytes"
                    | "retained_bytes" | "retained_objects" => (0.0, false),
                    "wall_time_us" | "exec_time_us" | "compile_time_us" | "gc_collection_us" => (0.50, true),
                    "ic_hit_rate" => (0.05, false),
                    _ => (0.20, false),
                };
                let ratio = cur_val / base_val;
                let regressed = if tolerance == 0.0 {
                    (ratio - 1.0).abs() > 1e-9
                } else if matches!(name, "ic_hit_rate") {
                    ratio < 1.0 - tolerance
                } else {
                    ratio > 1.0 + tolerance
                };
                if regressed {
                    regressions.push(Regression {
                        metric: name.to_string(),
                        test_name: cur.test_name.clone(),
                        baseline: base_val,
                        current: cur_val,
                        ratio,
                        tolerance,
                        informational,
                    });
                }
            }
        }
    }
    regressions
}
