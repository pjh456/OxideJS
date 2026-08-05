use crate::bench::metrics::MetricCollection;

/// 把结果序列化为美观 JSON 字符串。
pub fn format_json(results: &[MetricCollection]) -> String {
    serde_json::to_string_pretty(results).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
}

/// 把结果格式化为对齐的文本表格（供终端打印）。
pub fn format_text_table(results: &[MetricCollection]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<35} {:>10} {:>10} {:>10} {:>12} {:>8} {:>8} {:>10} {:>10}\n",
        "Test Name", "Wall(ms)", "Comp(ms)", "Exec(ms)", "Instrs", "GC Ct", "IC Rate", "Sess Objs", "Sess B",
    ));
    out.push_str(&"-".repeat(120));
    out.push('\n');

    for m in results {
        out.push_str(&format!(
            "{:<35} {:>10.2} {:>10.3} {:>10.3} {:>12} {:>8} {:>7.1}% {:>10} {:>10}",
            truncate(&m.test_name, 35),
            m.wall_time_us as f64 / 1000.0,
            m.compile_time_us as f64 / 1000.0,
            m.exec_time_us as f64 / 1000.0,
            m.instruction_count,
            m.gc_trigger_count,
            m.ic_hit_rate * 100.0,
            m.session_objects,
            m.session_bytes,
        ));
        out.push('\n');
    }
    out
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
