use crate::bench::metrics::MetricCollection;

/// 把结果序列化为美观 JSON 字符串。
pub fn format_json(results: &[MetricCollection]) -> String {
    serde_json::to_string_pretty(results).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
}

/// 内存基准用例（`mem_*` 前缀）的留存形态说明，随表格尾部输出。
const MEMORY_BENCH_NOTES: &str = "\n内存基准用例（mem_*）：\n
  mem_object_graph        大型对象图留存：15 层满二叉树 32767 节点整体驻留 session 堆
  mem_string_accum        字符串累积留存：40000 个互不相同的字符串驻留数组（interner 压力）
  mem_closure_chain       闭包/upvalue 长链：10000 层闭包各自捕获前层闭包与 payload 驻留
  mem_array_grow          数组增缩容量：扩到 200000 元素后截回 1000，观测元素向量容量是否回落（账目按 Vec capacity 计）
  mem_cross_call_retain   跨调用长存活数据：全局 store 经 30000 次调用累积 30000 条记录
  mem_map_retain          原生盒边持有：Map 持 20000 组字符串键 + 对象值
  mem_set_retain          原生盒边持有：Set 持 20000 个对象（各带 payload 字符串）
  mem_string_rope         拼接 rope 链留存：重复二元拼接形成深 Cons 链（账目按逻辑长度计，数值高于实际足迹）
口径：Peak B/Ret B 为 session 堆账目（对象头 + 堆数据 + 存活串/BigInt）；epoch arena
的临时驻留无字节账目（既有口径，数量见 JSON 的 epoch_objects）。Ret B 为 workload 后
晋升 + 完整 GC 的真存活集。
";

/// 把结果格式化为对齐的文本表格（供终端打印）。
///
/// `Sess B`/`Peak B`/`Ret B` 为 session 堆账目（字节）：执行期累计 / 峰值高水位 /
/// workload 后强制 GC 的存活留存。
pub fn format_text_table(results: &[MetricCollection]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<35} {:>10} {:>10} {:>10} {:>12} {:>8} {:>8} {:>10} {:>10} {:>10} {:>10}\n",
        "Test Name",
        "Wall(ms)",
        "Comp(ms)",
        "Exec(ms)",
        "Instrs",
        "GC Ct",
        "IC Rate",
        "Sess Objs",
        "Sess B",
        "Peak B",
        "Ret B",
    ));
    out.push_str(&"-".repeat(150));
    out.push('\n');

    for m in results {
        out.push_str(&format!(
            "{:<35} {:>10.2} {:>10.3} {:>10.3} {:>12} {:>8} {:>7.1}% {:>10} {:>10} {:>10} {:>10}",
            truncate(&m.test_name, 35),
            m.wall_time_us as f64 / 1000.0,
            m.compile_time_us as f64 / 1000.0,
            m.exec_time_us as f64 / 1000.0,
            m.instruction_count,
            m.gc_trigger_count,
            m.ic_hit_rate * 100.0,
            m.session_objects,
            m.session_bytes,
            m.peak_bytes,
            m.retained_bytes,
        ));
        out.push('\n');
    }
    out.push_str(MEMORY_BENCH_NOTES);
    out
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
