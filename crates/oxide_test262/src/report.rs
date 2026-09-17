//! 失败诊断：FailRecord 捕获、FAIL 清单/类别段格式化。
//! 全部为纯函数/String builder（可单测），不持有全局状态。

use crate::RunStats;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 单条失败记录：路径用全局测试数组下标（8 字节），类别用 RunStats 内 id 表下标。
/// 消息完整保留（单条 2 KiB cap + 全局 64 MiB cap），不截断到类别桶。
#[derive(Debug)]
pub struct FailRecord {
    pub index: usize,     // paths[index]
    pub category_id: u16, // RunStats.categories 下标
    pub subkey: String,   // not callable 调用点 / not defined 标识符 / 其余空串
    pub message: String,  // 完整错误文本（≤ 2048 字符）
    #[expect(dead_code)] // strict 矩阵预留，当前恒 0；启用后赋值
    pub scenario: u8, // 0 = default（strict 预留）
}

/// fail_records 累计 message 字节上限（超过后新记录降级为摘要，不累计字节）。
pub const MAX_FAIL_RECORD_BYTES: usize = 64 * 1024 * 1024;
/// 单条 message 长度上限（字符数）。
pub const MAX_FAIL_MSG_CHARS: usize = 2048;
/// FAIL 清单每类别保留样本条数。
pub const FAIL_LIST_SAMPLES_PER_CATEGORY: usize = 5;

/// 取消息首行（FAIL 清单 / 类别样本展示用）。
pub fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// 压平 \t \n \r 为空格（fail-log / 心跳旁路行 / 类别行的可解析保证）。
pub fn escape_log_field(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// 类别计数降序的打印版（返回 String，可单测）；`vm: other`/`compile: other`/`other`
/// 碎片桶附带 ≤5 条样本（`路径  消息首行`）。空类别返回空串。
pub fn format_fail_categories(stats: &RunStats, paths: &[PathBuf]) -> String {
    if stats.fail_categories.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("  --- FAIL categories ---\n");
    let mut cats: Vec<_> = stats.fail_categories.iter().collect();
    cats.sort_by_key(|(_, c)| -(**c as isize));
    for (cat, count) in cats {
        out.push_str(&format!("    {:>4}  {}\n", count, cat));
        // 碎片桶附 ≤5 条样本（路径 + 消息首行），供报告快速定位。
        if cat.starts_with("vm: other") || cat.starts_with("compile: other") || cat.starts_with("other") {
            for rec in stats
                .fail_records
                .iter()
                .filter(|r| stats.categories[r.category_id as usize] == *cat)
                .take(FAIL_LIST_SAMPLES_PER_CATEGORY)
            {
                let path = paths
                    .get(rec.index)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| format!("#{}", rec.index));
                out.push_str(&format!("           sample: {path}  {}\n", first_line(&rec.message)));
            }
        }
    }
    out
}

/// `--- FAIL list (N) ---`：每类别 ≤ FAIL_LIST_SAMPLES_PER_CATEGORY 条
/// `FAIL <path> [<category>] <message 首行>`，其余按计数折叠。无记录返回空串。
pub fn format_fail_list(stats: &RunStats, paths: &[PathBuf]) -> String {
    if stats.fail_records.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!("  --- FAIL list ({}) ---\n", stats.fail_records.len()));
    let mut cats: Vec<_> = stats.fail_categories.iter().collect();
    cats.sort_by_key(|(_, c)| -(**c as isize));
    for (cat, _) in cats {
        let recs: Vec<_> = stats
            .fail_records
            .iter()
            .filter(|r| stats.categories[r.category_id as usize] == *cat)
            .collect();
        for rec in recs.iter().take(FAIL_LIST_SAMPLES_PER_CATEGORY) {
            let path = paths
                .get(rec.index)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| format!("#{}", rec.index));
            out.push_str(&format!("    FAIL {path} [{cat}] {}\n", first_line(&rec.message)));
        }
        let rest = recs.len().saturating_sub(FAIL_LIST_SAMPLES_PER_CATEGORY);
        if rest > 0 {
            out.push_str(&format!("    (+{rest} more in {cat})\n"));
        }
    }
    out
}

/// 从 `TypeError: <X> is not callable` 提取调用点 X。
///
/// 定位首个 `TypeError: ` 前缀，取其后到 ` is not callable` 后缀之间的文本；
/// 无前缀 / 非该后缀返回 `"(none)"`（不误伤类别判定）。
///
/// # 边界与前提
/// - `msg` 可带 `vm error: ` 等前导前缀，`find("TypeError: ")` 定位不受影响。
/// - 提取失败（无前缀 / 无后缀 / 中间空）统一归 `"(none)"`，仍落
///   `vm: not callable` 大类桶，不改变统计口径。
pub fn extract_not_callable_subkey(msg: &str) -> String {
    let Some(start) = msg.find("TypeError: ") else { return "(none)".into() };
    let rest = &msg[start + "TypeError: ".len()..];
    let Some(end) = rest.find(" is not callable") else { return "(none)".into() };
    let x = rest[..end].trim();
    if x.is_empty() {
        "(none)".into()
    } else {
        x.to_string()
    }
}

/// 把一条失败记录追加到 supervise 旁路文件（子进程 → 父进程通道）。
/// 行格式 `index\tcategory\tsubkey\tmessage`，字段经 escape_log_field 压平。
///
/// # 注意事项
/// - 追加写不做原子性：父进程只在子进程死亡后才读取，半写尾行由
///   [`parse_fail_log`] 防御性跳过。
pub fn append_fail_log(path: &Path, index: usize, category: &str, subkey: &str, message: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(
        f,
        "{}\t{}\t{}\t{}",
        index,
        escape_log_field(category),
        escape_log_field(subkey),
        escape_log_field(message)
    )
}

/// 解析旁路失败行文件为 `(index, category, subkey, message)` 列表。
///
/// # 边界与前提
/// - 残缺尾行（子进程被杀时半写）与畸形行（字段数不足 / index 非数字）静默跳过。
pub fn parse_fail_log(content: &str) -> Vec<(usize, String, String, String)> {
    let mut out = Vec::new();
    for line in content.lines() {
        let mut it = line.splitn(4, '\t');
        let (Some(index), Some(cat), Some(subkey), Some(message)) = (it.next(), it.next(), it.next(), it.next()) else {
            continue;
        };
        let Ok(index) = index.parse::<usize>() else { continue };
        out.push((index, cat.to_string(), subkey.to_string(), message.to_string()));
    }
    out
}

/// 目录分组键：`path.parent()` 组件的末 3 级以 `/` 拼接；
/// 不足 3 级时取全部可用组件（无目录的文件返回空串）。
fn directory_group_key(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    let comps: Vec<String> = parent
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps[comps.len().saturating_sub(3)..].join("/")
}

/// 目录分组 + subkey 子分组聚合打印：目录 top 30 + subkey top 20，超出折叠。
///
/// # 步骤
/// 1. 目录分组：各记录 `path.parent()` 末 3 级组件为键，计数降序取前 30。
/// 2. subkey 子分组：仅展开 `vm: not callable` / `vm: not defined` /
///    `compile: not defined` 三类（计数降序取前 20）；`vm: IC_GET_PROP on
///    non-object` subkey 恒空（靠目录分组区分），不展开；空 subkey 显示为 `(none)`。
///
/// # 边界与前提
/// - 空 fail_records 返回空串；记录下标越出 paths 范围者不参与目录分组。
/// - 返回串自带尾换行，调用方原样打印。
pub fn format_fail_groupings(stats: &RunStats, paths: &[PathBuf]) -> String {
    if stats.fail_records.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("  --- FAIL groupings ---\n");

    // 目录分组：按父路径末 3 级聚合，计数降序、同数按键升序。
    let mut dirs: HashMap<String, usize> = HashMap::new();
    for rec in &stats.fail_records {
        let Some(path) = paths.get(rec.index) else {
            continue;
        };
        *dirs.entry(directory_group_key(path)).or_insert(0) += 1;
    }
    let mut dirs: Vec<_> = dirs.into_iter().collect();
    dirs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.push_str(&format!("    directory ({})\n", dirs.len()));
    for (key, count) in dirs.iter().take(30) {
        out.push_str(&format!("      {key} : {count}\n"));
    }
    let rest = dirs.len().saturating_sub(30);
    if rest > 0 {
        out.push_str(&format!("      (+{rest} more directories)\n"));
    }

    // subkey 子分组：仅展开三类带真 subkey 的类别，空 subkey 显示 (none)。
    let mut subs: HashMap<(String, String), usize> = HashMap::new();
    for rec in &stats.fail_records {
        let Some(cat) = stats.categories.get(rec.category_id as usize) else {
            continue;
        };
        if !matches!(cat.as_str(), "vm: not callable" | "vm: not defined" | "compile: not defined") {
            continue;
        }
        let subkey = if rec.subkey.is_empty() { "(none)".to_string() } else { rec.subkey.clone() };
        *subs.entry((cat.clone(), subkey)).or_insert(0) += 1;
    }
    let mut subs: Vec<_> = subs.into_iter().map(|((cat, subkey), count)| (cat, subkey, count)).collect();
    subs.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)).then_with(|| a.1.cmp(&b.1)));
    for (cat, subkey, count) in subs.iter().take(20) {
        out.push_str(&format!("    subkey: {cat} ({subkey}) : {count}\n"));
    }
    let rest = subs.len().saturating_sub(20);
    if rest > 0 {
        out.push_str(&format!("    (+{rest} more subkeys)\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::TestResult;

    #[test]
    fn first_line_trims_at_newline() {
        assert_eq!(first_line("abc\ndef"), "abc");
        assert_eq!(first_line("abc\r\n"), "abc");
    }

    #[test]
    fn escape_log_field_flattens_separators() {
        assert_eq!(escape_log_field("a\tb\nc\rd"), "a b c d");
    }

    /// `not callable` 调用点提取：带/不带 `vm error: ` 前缀均可定位，
    /// 无前缀 / 无后缀 / 中间空一律回退 `(none)`。
    #[test]
    fn extract_not_callable_subkey_extracts_call_site() {
        assert_eq!(extract_not_callable_subkey("TypeError: CALL target is not callable"), "CALL target");
        assert_eq!(extract_not_callable_subkey("vm error: TypeError: x is not callable"), "x");
        assert_eq!(extract_not_callable_subkey("TypeError: not callable"), "(none)");
        assert_eq!(extract_not_callable_subkey("boom"), "(none)");
    }

    /// 分组聚合：目录分组（末 3 级键）计数正确、subkey 仅三类展开且按 subkey 分桶、
    /// IC_GET_PROP 不展开、目录 top N 折叠行出现、空记录返回空串。
    #[test]
    fn format_fail_groupings_groups_by_directory_and_subkey() {
        let mut stats = RunStats::default();
        let mut paths: Vec<PathBuf> = Vec::new();
        // 33 个不同目录（超 top-30 上限），每目录一条 not callable 记录，
        // subkey 交替取两个值，验证按 subkey 分桶。
        for i in 0..32 {
            paths.push(PathBuf::from(format!("language/expressions/dir{i:02}/t.js")));
            let subkey = if i % 2 == 0 { "CALL target" } else { "accessor" };
            stats.push_fail_record(i, "vm: not callable".into(), subkey.into(), "m".into());
        }
        // IC_GET_PROP：subkey 恒空，不应产生 subkey 行。
        paths.push(PathBuf::from("built-ins/Symbol/prototype/s.js"));
        stats.push_fail_record(32, "vm: IC_GET_PROP on non-object".into(), String::new(), "m".into());
        let out = format_fail_groupings(&stats, &paths);
        assert!(out.contains("language/expressions/dir00 : 1"), "实际:\n{out}");
        assert!(out.contains("built-ins/Symbol/prototype : 1"), "实际:\n{out}");
        assert!(out.contains("(+3 more directories)"), "实际:\n{out}");
        assert!(out.contains("subkey: vm: not callable (CALL target) : 16"), "实际:\n{out}");
        assert!(out.contains("subkey: vm: not callable (accessor) : 16"), "实际:\n{out}");
        assert!(!out.contains("IC_GET_PROP"), "IC_GET_PROP 不应展开 subkey 行，实际:\n{out}");
        // 空 stats 返回空串。
        assert_eq!(format_fail_groupings(&RunStats::default(), &paths), "");
    }

    /// 目录键取父路径末 3 级：3 级父路径原样、更深父路径截末 3 级、
    /// 不足 3 级取全部可用组件（无目录文件为空键）。
    #[test]
    fn format_fail_groupings_uses_last_3_parent_components() {
        let mut stats = RunStats::default();
        let paths = vec![
            PathBuf::from("language/expressions/addition/b1.js"),
            PathBuf::from("a/b/c/d/e.js"),
            PathBuf::from("top.js"),
        ];
        for i in 0..paths.len() {
            stats.push_fail_record(i, "vm: not defined".into(), "foo".into(), "m".into());
        }
        let out = format_fail_groupings(&stats, &paths);
        assert!(out.lines().any(|l| l.trim_start() == "language/expressions/addition : 1"), "实际:\n{out}");
        assert!(out.lines().any(|l| l.trim_start() == "b/c/d : 1"), "实际:\n{out}");
        assert!(out.lines().any(|l| l.trim_start() == ": 1"), "实际:\n{out}");
    }

    /// FAIL 清单每类别只列样本条数，其余按折叠行计数。
    #[test]
    fn format_fail_list_folds_over_limit() {
        let mut stats = RunStats::default();
        let paths: Vec<PathBuf> = (0..7).map(|i| PathBuf::from(format!("p{i}.js"))).collect();
        for (i, p) in paths.iter().enumerate() {
            stats.record(i, &TestResult::fail(p.clone(), 1, "vm error: x is not callable"));
        }
        let out = format_fail_list(&stats, &paths);
        let fail_lines = out.lines().filter(|l| l.starts_with("    FAIL ")).count();
        assert_eq!(fail_lines, 5);
        assert!(out.contains("(+2 more in vm: not callable)"), "应含折叠行，实际:\n{out}");
    }

    /// 碎片桶附样本行：路径 + 消息首行，类别带截断尾巴也能前缀匹配。
    #[test]
    fn format_fail_categories_attaches_other_bucket_samples() {
        let mut stats = RunStats::default();
        let paths = vec![PathBuf::from("p.js")];
        stats.record(0, &TestResult::fail(paths[0].clone(), 1, "vm error: weird message one two"));
        let out = format_fail_categories(&stats, &paths);
        assert!(out.contains("sample:"), "应含样本行，实际:\n{out}");
        assert!(out.contains("weird message one"), "样本应含消息首行，实际:\n{out}");
    }

    /// 无失败记录时 FAIL 清单段为空串（调用处静默）。
    #[test]
    fn format_fail_list_empty_without_records() {
        let stats = RunStats::default();
        let paths: Vec<PathBuf> = Vec::new();
        assert_eq!(format_fail_list(&stats, &paths), "");
        assert_eq!(format_fail_categories(&stats, &paths), "");
    }

    /// 旁路失败行往返：append 后 parse 恢复全部字段；残缺/畸形尾行静默跳过。
    #[test]
    fn fail_log_round_trips_and_skips_truncated() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_fails_test_{}.log", std::process::id()));
        append_fail_log(&path, 3, "vm: not callable", "", "x is not callable").expect("追加失败");
        append_fail_log(&path, 4, "compile: unsupported", "foo", "a\tb\nc").expect("追加失败");
        let mut content = std::fs::read_to_string(&path).expect("读取失败");
        content.push_str("5\tvm: x\n"); // 残缺：仅 2 字段（SIGKILL 半写形态）
        content.push_str("x\tb\tc\td\n"); // 畸形：index 非数字
        std::fs::write(&path, content).expect("写回失败");
        let content = std::fs::read_to_string(&path).expect("读取失败");
        let rows = parse_fail_log(&content);
        assert_eq!(rows.len(), 2, "残缺/畸形行应被跳过，实际 {rows:?}");
        assert_eq!(
            rows[0],
            (3, "vm: not callable".to_string(), "".to_string(), "x is not callable".to_string())
        );
        assert_eq!(rows[1], (4, "compile: unsupported".to_string(), "foo".to_string(), "a b c".to_string()));
        let _ = std::fs::remove_file(&path);
    }
}
