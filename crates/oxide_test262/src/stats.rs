//! 全部测试运行的累计统计：通过/失败/跳过计数、总耗时、失败原因分类表
//! 与逐路径失败记录；支持合并多 worker/子进程的部分统计。
//!
//! 类别 id 表（categories）是 `FailRecord.category_id` 的唯一编号源，合并时
//! fail_records 按目标侧表重映射 id；单条失败消息 cap 2 KiB，累计字节 cap
//! 64 MiB（超限降级首行摘要）。

use crate::judge::{categorize_fail, TestOutcome, TestResult};
use crate::report::{first_line, FailRecord, MAX_FAIL_MSG_CHARS, MAX_FAIL_RECORD_BYTES};
use std::collections::HashMap;

/// 全部已运行测试的累计统计：通过/失败/跳过计数、总耗时、失败原因分类
/// 与逐路径失败记录（fail_records，供汇总区聚合报告）。
#[derive(Default)]
pub(crate) struct RunStats {
    pub(crate) pass: usize,
    pub(crate) fail: usize,
    pub(crate) skip: usize,
    pub(crate) total_ms: u64,
    pub(crate) fail_categories: HashMap<String, usize>, // 类别名是心跳序列化格式与基线报告的契约键，改动会使旧心跳行对不上
    pub(crate) categories: Vec<String>,                 // 类别 id 表（FailRecord.category_id 索引）
    pub(crate) fail_records: Vec<FailRecord>,
    pub(crate) fail_record_bytes: usize, // 累计 message 字节（OOM cap）
    // supervise 模式异常计数：spawn 失败 / try_wait 错误 / 心跳写失败（子进程侧
    // 累计后经心跳第 6 字段回传）。
    pub(crate) spawn_errors: usize,
    pub(crate) wait_errors: usize,
    pub(crate) hb_write_errors: usize,
    /// 超时/崩溃清单：(index, elapsed_ms)；elapsed_ms=0 表示非超时崩溃。
    pub(crate) timeout_crashes: Vec<(usize, u64)>,
}

impl RunStats {
    /// 把另一个 worker 的部分统计并入本对象。用于并行执行后把各 worker 的
    /// 结果合并回单一总计。失败记录的类别 id 按本表重映射（两侧类别表独立编号）。
    pub(crate) fn merge(&mut self, other: RunStats) {
        self.pass += other.pass;
        self.fail += other.fail;
        self.skip += other.skip;
        self.total_ms += other.total_ms;
        for (cat, count) in other.fail_categories {
            *self.fail_categories.entry(cat).or_insert(0) += count;
        }
        for rec in other.fail_records {
            let name = other.categories[rec.category_id as usize].clone();
            let id = self.category_id_of(&name);
            self.fail_records.push(FailRecord { category_id: id, ..rec });
        }
        self.fail_record_bytes += other.fail_record_bytes;
        self.spawn_errors += other.spawn_errors;
        self.wait_errors += other.wait_errors;
        self.hb_write_errors += other.hb_write_errors;
        self.timeout_crashes.extend(other.timeout_crashes);
    }

    /// 把单个测试结果记入运行累计；失败同时追加逐路径失败记录（含类别）。
    pub(crate) fn record(&mut self, index: usize, result: &TestResult) {
        match &result.outcome {
            TestOutcome::Pass(_) => self.pass += 1,
            TestOutcome::Fail(msg) => {
                let (cat, subkey) = categorize_fail(msg);
                *self.fail_categories.entry(cat.clone()).or_insert(0) += 1;
                self.push_fail_record(index, cat, subkey, msg.clone());
                self.fail += 1;
            }
            TestOutcome::Skip(_) => self.skip += 1,
        }
        self.total_ms += result.duration_ms;
    }

    /// 取得类别名在 categories 表中的 id（不存在则追加）。
    fn category_id_of(&mut self, name: &str) -> u16 {
        debug_assert!(self.categories.len() < u16::MAX as usize, "categories 表超出 u16 容量");
        if let Some(pos) = self.categories.iter().position(|c| c == name) {
            return pos as u16;
        }
        self.categories.push(name.to_string());
        (self.categories.len() - 1) as u16
    }

    /// 追加一条失败记录：单条消息截断到 2 KiB；累计字节超 64 MiB 后本条
    /// 降级为消息首行摘要且不再累计字节，防止 OOM。
    pub(crate) fn push_fail_record(&mut self, index: usize, category: String, subkey: String, message: String) {
        let message = message.chars().take(MAX_FAIL_MSG_CHARS).collect::<String>();
        let bytes = message.len();
        let category_id = self.category_id_of(&category);
        let message = if self.fail_record_bytes + bytes <= MAX_FAIL_RECORD_BYTES {
            self.fail_record_bytes += bytes;
            message
        } else {
            // 累计超上限：本条降级为消息首行摘要，不再累计字节。
            first_line(&message).chars().take(120).collect::<String>()
        };
        self.fail_records.push(FailRecord {
            index,
            category_id,
            subkey,
            message,
            scenario: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// record 把真 subkey 写入 FailRecord（not callable 调用点透传）。
    #[test]
    fn record_stores_real_subkey() {
        let mut stats = RunStats::default();
        stats.record(
            0,
            &TestResult::fail(PathBuf::from("p.js"), 1, "vm error: TypeError: Map.set is not callable"),
        );
        assert_eq!(stats.fail_records.len(), 1);
        assert_eq!(stats.fail_records[0].subkey, "Map.set");
    }

    /// record 追加失败记录：index 透传、类别正确、消息完整保留。
    #[test]
    fn runstats_record_appends_fail_record() {
        let mut stats = RunStats::default();
        let result = TestResult::fail(PathBuf::from("a.js"), 7, "vm error: x is not callable");
        stats.record(5, &result);
        assert_eq!(stats.fail, 1);
        assert_eq!(stats.fail_records.len(), 1);
        assert_eq!(stats.fail_records[0].index, 5);
        let cat = &stats.categories[stats.fail_records[0].category_id as usize];
        assert_eq!(cat, "vm: not callable");
        assert_eq!(stats.fail_records[0].message, "vm error: x is not callable");
    }

    /// 超长失败消息按字符截断到单条上限。
    #[test]
    fn runstats_record_caps_message_length() {
        let mut stats = RunStats::default();
        let result = TestResult::fail(PathBuf::from("b.js"), 1, "x".repeat(5000));
        stats.record(0, &result);
        assert!(stats.fail_records[0].message.chars().count() <= MAX_FAIL_MSG_CHARS);
    }

    /// merge 拼接失败记录并重映射类别 id：同类别共享一个 id，字节计数为各侧之和。
    #[test]
    fn runstats_merge_concats_fail_records_with_id_remap() {
        let mut a = RunStats::default();
        a.record(0, &TestResult::fail(PathBuf::from("x.js"), 1, "vm error: a is not callable"));
        a.record(1, &TestResult::fail(PathBuf::from("y.js"), 1, "vm error: b is not callable"));
        let mut b = RunStats::default();
        b.record(2, &TestResult::fail(PathBuf::from("z.js"), 1, "vm error: c is not callable"));
        a.merge(b);
        assert_eq!(a.fail, 3);
        assert_eq!(a.fail_records.len(), 3);
        let id0 = a.fail_records[0].category_id;
        assert_eq!(a.fail_records[1].category_id, id0);
        assert_eq!(a.fail_records[2].category_id, id0);
        assert_eq!(a.categories[id0 as usize], "vm: not callable");
        assert_eq!(a.fail_record_bytes, a.fail_records.iter().map(|r| r.message.len()).sum::<usize>());
    }

    /// merge 合并异常计数器：spawn/wait/hb 求和、timeout_crashes 拼接。
    #[test]
    fn runstats_merge_sums_error_counters() {
        let mut a = RunStats {
            spawn_errors: 1,
            wait_errors: 2,
            hb_write_errors: 3,
            timeout_crashes: vec![(0, 100)],
            ..RunStats::default()
        };
        let b = RunStats {
            spawn_errors: 4,
            wait_errors: 5,
            hb_write_errors: 6,
            timeout_crashes: vec![(1, 200)],
            ..RunStats::default()
        };
        a.merge(b);
        assert_eq!(a.spawn_errors, 5);
        assert_eq!(a.wait_errors, 7);
        assert_eq!(a.hb_write_errors, 9);
        assert_eq!(a.timeout_crashes, vec![(0, 100), (1, 200)]);
    }
}
