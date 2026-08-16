//! 失败诊断：FailRecord 捕获、FAIL 清单/类别段格式化。
//! 全部为纯函数/String builder（可单测），不持有全局状态。

use crate::RunStats;
use std::path::{Path, PathBuf};

/// 单条失败记录：路径用全局测试数组下标（8 字节），类别用 RunStats 内 id 表下标。
/// 消息完整保留（单条 2 KiB cap + 全局 64 MiB cap），不截断到类别桶。
#[derive(Debug)]
pub struct FailRecord {
    pub index: usize,     // paths[index]
    pub category_id: u16, // RunStats.categories 下标
    #[expect(dead_code)] // 当前恒空串；subkey 分组读取落地后填充真值
    pub subkey: String, // not callable 调用点 / not defined 标识符 / 其余空串
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

/// 原 print_fail_categories 的 String 版：类别计数降序；`vm: other`/`compile: other`
/// /`other` 碎片桶附带 ≤5 条样本（`路径  消息首行`）。空类别返回空串。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_trims_at_newline() {
        assert_eq!(first_line("abc\ndef"), "abc");
        assert_eq!(first_line("abc\r\n"), "abc");
    }

    #[test]
    fn escape_log_field_flattens_separators() {
        assert_eq!(escape_log_field("a\tb\nc\rd"), "a b c d");
    }
}
