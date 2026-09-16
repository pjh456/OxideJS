//! 监督模式心跳协议：子进程每完成一个测试覆写心跳文件（`.tmp` + rename
//! 原子落盘），父进程轮询读回并入累计统计。
//!
//! 头为 6 字段（phase index pass fail skip hb_write_errors），其后逐行失败类别
//! `类别\t计数`；旧 5 字段 `START` 格式宽容映射为 `COMPLETED(index - 1)`；
//! 第 6 字段把子进程侧心跳/旁路写失败累计回传父进程。

use crate::stats::RunStats;
use std::collections::HashMap;
use std::path::Path;

/// 监督模式下子进程写入、父进程轮询的一条心跳记录。
/// `COMPLETED`：每个测试完成后写，`index` 为刚完成的全局测试下标，计数覆盖
/// ≤ index 的全部测试；`DONE`：窗口尾，`index` 为窗口结束下标。
/// `categories` 为子进程失败分类计数快照（首行之后按 `类别\t计数` 逐行写出）；
/// `hb_write_errors` 为子进程侧心跳/旁路写失败累计，经第 6 字段回传父进程。
pub(crate) struct Heartbeat {
    pub(crate) phase: String,
    pub(crate) index: usize,
    pub(crate) pass: usize,
    pub(crate) fail: usize,
    pub(crate) skip: usize,
    pub(crate) categories: HashMap<String, usize>,
    pub(crate) hb_write_errors: usize, // 子进程侧写失败累计（第 6 字段）
}

/// 用单行心跳头（phase index pass fail skip hb_write_errors）+ 失败类别行覆写心跳文件。
///
/// 经 `{path}.tmp` 临时文件再 rename 原子落盘（同目录 POSIX 原子替换；不 fsync，
/// SIGKILL 下 page cache 幸存，ponytail）。写失败返回 Err 并清理临时文件，由调用方
/// 自增 hb_write_errors 随下一心跳回传父进程。
///
/// # 边界与前提
/// - 假定单 worker（监督器强制 `OXIDE_TEST262_WORKERS=1`）；多 worker 时运行下标
///   有歧义且对同一路径的覆写存在竞争。
///
/// # 副作用
/// - 覆写 `path`；失败时可能残留 `path.tmp`（调用方或 supervise 清理兜底）。
#[expect(clippy::too_many_arguments)]
pub(crate) fn write_heartbeat(
    path: &Path, phase: &str, index: usize, pass: usize, fail: usize, skip: usize, categories: &HashMap<String, usize>,
    hb_write_errors: usize,
) -> std::io::Result<()> {
    let mut content = format!("{phase} {index} {pass} {fail} {skip} {hb_write_errors}\n");
    for (cat, count) in categories {
        // 类别文本内的制表符/换行会破坏行格式，写盘前压平。
        let cat = cat.replace(['\t', '\n', '\r'], " ");
        content.push_str(&format!("{cat}\t{count}\n"));
    }
    let tmp = format!("{}.tmp", path.display());
    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

/// 读取最新心跳（含失败类别行与第 6 字段写失败计数）。任何缺失/残缺/畸形内容
/// 均返回 `None`，使轮询循环可直接在下一拍重试。
///
/// # 边界与前提
/// - 旧 5 字段 `START` 格式宽容映射为 `COMPLETED(index - 1)`：旧 START(j) 计数
///   覆盖 < j 的测试，与 COMPLETED(j-1) 语义等价；index=0 时 saturating_sub 防下溢。
pub(crate) fn read_heartbeat(path: &Path) -> Option<Heartbeat> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let line = lines.next()?;
    let mut parts = line.split_whitespace();
    let phase = parts.next()?.to_string();
    let index: usize = parts.next()?.parse().ok()?;
    let pass = parts.next()?.parse().ok()?;
    let fail = parts.next()?.parse().ok()?;
    let skip = parts.next()?.parse().ok()?;
    let hb_write_errors = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut categories = HashMap::new();
    for l in lines {
        if let Some((cat, count)) = l.split_once('\t') {
            if let Ok(c) = count.trim().parse() {
                categories.insert(cat.to_string(), c);
            }
        }
    }
    let (phase, index) = if phase == "START" {
        ("COMPLETED".into(), index.saturating_sub(1))
    } else {
        (phase, index)
    };
    Some(Heartbeat {
        phase,
        index,
        pass,
        fail,
        skip,
        categories,
        hb_write_errors,
    })
}

/// 把心跳快照并入累计统计（含失败类别与子进程侧写失败计数）。
pub(crate) fn merge_heartbeat(stats: &mut RunStats, hb: &Heartbeat) {
    stats.pass += hb.pass;
    stats.fail += hb.fail;
    stats.skip += hb.skip;
    stats.hb_write_errors += hb.hb_write_errors;
    for (cat, count) in &hb.categories {
        *stats.fail_categories.entry(cat.clone()).or_insert(0) += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 心跳写读往返：类别行随心跳头一起持久化并完整还原（含制表符/换行压平），
    /// 第 6 字段 hb_write_errors 同步往返。
    #[test]
    fn heartbeat_round_trips_categories() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_{}.txt", std::process::id()));
        let mut categories = HashMap::new();
        categories.insert("vm: not defined".to_string(), 3);
        categories.insert("compile: unsupported".to_string(), 1);
        write_heartbeat(&path, "DONE", 42, 30, 4, 8, &categories, 3).expect("心跳写失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.phase, "DONE");
        assert_eq!(hb.index, 42);
        assert_eq!(hb.pass, 30);
        assert_eq!(hb.fail, 4);
        assert_eq!(hb.skip, 8);
        assert_eq!(hb.hb_write_errors, 3);
        assert_eq!(hb.categories.get("vm: not defined"), Some(&3));
        assert_eq!(hb.categories.get("compile: unsupported"), Some(&1));
        let _ = std::fs::remove_file(&path);
    }

    /// 类别文本含制表符/换行时写入压平，读取不破坏行结构。
    #[test]
    fn heartbeat_flattens_category_separators() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test2_{}.txt", std::process::id()));
        let mut categories = HashMap::new();
        categories.insert("vm: other (multi\nline\tmessage)".to_string(), 2);
        write_heartbeat(&path, "COMPLETED", 7, 1, 2, 3, &categories, 0).expect("心跳写失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.fail, 2);
        assert_eq!(hb.categories.len(), 1);
        let key = hb.categories.keys().next().unwrap();
        assert!(!key.contains('\t') && !key.contains('\n'), "类别键应已压平，实际 {key:?}");
        assert_eq!(hb.categories.get(key), Some(&2));
        let _ = std::fs::remove_file(&path);
    }

    /// 心跳第 6 字段（hb_write_errors）写读往返。
    #[test]
    fn heartbeat_round_trips_write_errors_field() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_wr_{}.txt", std::process::id()));
        let categories = HashMap::new();
        write_heartbeat(&path, "COMPLETED", 9, 5, 1, 2, &categories, 5).expect("心跳写失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.phase, "COMPLETED");
        assert_eq!(hb.index, 9);
        assert_eq!(hb.hb_write_errors, 5);
        let _ = std::fs::remove_file(&path);
    }

    /// 旧 5 字段 START 心跳宽容映射：START(j) → COMPLETED(j-1)；j=0 不溢出。
    #[test]
    fn read_heartbeat_maps_legacy_start_to_completed() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_legacy_{}.txt", std::process::id()));
        std::fs::write(&path, "START 7 1 2 3\n").expect("写原始心跳行失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.phase, "COMPLETED");
        assert_eq!(hb.index, 6);
        std::fs::write(&path, "START 0 0 0 0\n").expect("写原始心跳行失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.index, 0, "saturating_sub 防下溢");
        let _ = std::fs::remove_file(&path);
    }

    /// 原子写：写后目标文件内容完整、无 `.tmp` 残留。
    #[test]
    fn write_heartbeat_atomic_no_tmp_leftover() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_atomic_{}.txt", std::process::id()));
        let categories = HashMap::new();
        write_heartbeat(&path, "DONE", 10, 3, 2, 1, &categories, 0).expect("心跳写失败");
        let content = std::fs::read_to_string(&path).expect("心跳应可读回");
        assert!(content.starts_with("DONE 10 3 2 1 0\n"), "内容应为 6 字段头，实际:\n{content}");
        let tmp = format!("{}.tmp", path.display());
        assert!(!Path::new(&tmp).exists(), "tmp 文件不应残留");
        let _ = std::fs::remove_file(&path);
    }
}
