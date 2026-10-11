//! 监督窗口传输抽象：把 supervise 子进程窗口与父进程之间的文件协议（心跳文件、
//! 旁路失败行、last-pc 现场、sidecar 日志、子进程派生与杀死）收口成六操作传输
//! 接口；LocalFile 实现即现状文件协议逐位不变，远程模式将来加 Tcp 实现。
//!
//! 传输只负责「怎么把窗口跑起来、怎么读回状态」；超时判定、重启循环、记账等
//! 监督策略留在 supervise 侧。传输实例是窗口级状态：跨子进程重启存活，
//! `assign` 只重置子进程句柄，不重置已消费偏移。

use crate::heartbeat::{read_heartbeat, Heartbeat};
use crate::report::parse_fail_log;
use crate::stats::RunStats;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

/// 派生窗口子进程失败：spawn 的 io 错误。
pub(crate) struct SpawnError(std::io::Error);

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 子进程等待三态：死亡（带退出状态）/ 存活 / 等待错误。
/// 三臂在监督策略里处理不同（死亡走心跳裁决、存活继续轮询、错误走 kill 加
/// 记账），不得压扁。
pub(crate) enum PollResult {
    Exited(ExitStatus),
    Alive,
    WaitError(std::io::Error),
}

/// 监督窗口传输：六操作（assign/heartbeat/merge_fail/scene/kill/poll）加
/// 生命周期与日志辅助。LocalFile 臂是现状文件协议，远程臂将来是窗口指派/
/// 心跳/失败记录/现场/kill/DONE 帧。
pub(crate) trait WindowTransport {
    /// 派生一个跑 `[wstart, wend)` 的子进程（LocalFile：九项环境变量加
    /// sidecar 双写，打开失败回退丢弃不中断）。
    fn assign(&mut self, wstart: usize, wend: usize) -> Result<(), SpawnError>;
    /// 读最新心跳（缺失/畸形返回 None，调用方下一拍重试）。
    fn heartbeat(&self) -> Option<Heartbeat>;
    /// 把旁路失败行新增段并入统计（已消费偏移跨重启保留，防双计）。
    fn merge_fail(&mut self, stats: &mut RunStats);
    /// 读子进程被杀前留下的 last-pc 现场行（缺文件/残行返回 None）。
    fn scene(&self) -> Option<String>;
    /// 杀死子进程并等待回收。
    fn kill(&mut self);
    /// 非阻塞查询子进程状态（三态）。
    fn poll(&mut self) -> PollResult;
    /// 读 sidecar 日志末 ~20 行（超时/崩溃后供现场重建）。
    fn log_tail(&self) -> Option<String>;
    /// 每轮重启前清理：删心跳文件、其 .tmp 与 last-pc 现场文件；旁路失败行
    /// 与 sidecar 日志跨重启追加保留。
    fn reset_round(&mut self);
    /// 窗口收尾：删心跳与 .tmp 与 last-pc 文件，旁路失败行与 sidecar 日志
    /// 归档为 .done 而非删除（收尾逐文件差分的终态依据）。
    fn archive_window(&mut self);
}

/// 本地文件传输：现状文件协议实现（心跳文件 + 旁路失败行 + last-pc 现场 +
/// sidecar 日志 + 子进程句柄）。
pub(crate) struct LocalFileWindowTransport {
    exe: PathBuf,
    args: Vec<String>,
    paths_file: PathBuf,
    hb_path: PathBuf,
    log_path: PathBuf,
    pc_path: PathBuf,
    /// 旁路失败行已消费字节偏移：窗口级状态，跨子进程重启保留（防双计）。
    fails_consumed: usize,
    child: Option<Child>,
}

impl LocalFileWindowTransport {
    /// 构造传输：按 pid 加 window_id 命名规则算好三个文件路径（与现状一致）。
    pub(crate) fn new(exe: &Path, args: &[String], paths_file: &Path, window_id: usize) -> Self {
        let hb_path = std::env::temp_dir().join(format!("oxide_t262_hb_{}_{}.txt", std::process::id(), window_id));
        let log_path = hb_path.with_extension("log");
        let pc_path = std::env::temp_dir().join(format!("oxide_t262_pc_{}_{}.txt", std::process::id(), window_id));
        Self {
            exe: exe.to_path_buf(),
            args: args.to_vec(),
            paths_file: paths_file.to_path_buf(),
            hb_path,
            log_path,
            pc_path,
            fails_consumed: 0,
            child: None,
        }
    }
}

impl WindowTransport for LocalFileWindowTransport {
    fn assign(&mut self, wstart: usize, wend: usize) -> Result<(), SpawnError> {
        let max_tests = wend - wstart;
        // sidecar 日志经手：子进程 stdout/stderr 双写此文件（直接文件写，无
        // pipe 缓冲死锁面）；打开失败回退丢弃，不中断运行。
        let (stdout, stderr) = match std::fs::OpenOptions::new().create(true).append(true).open(&self.log_path) {
            Ok(file) => {
                // 文件经手克隆：底层 fd 复制，stdout/stderr 各持一份。
                let err_file = file.try_clone().expect("sidecar 日志经手克隆失败");
                let out = std::process::Stdio::from(file);
                let err = std::process::Stdio::from(err_file);
                (out, err)
            }
            Err(_) => (std::process::Stdio::null(), std::process::Stdio::null()),
        };

        let child = Command::new(&self.exe)
            .args(self.args.iter().skip(1))
            .env("OXIDE_SKIP_UNTIL", wstart.to_string())
            .env("OXIDE_MAX_TESTS", max_tests.to_string())
            .env("OXIDE_TEST262_WORKERS", "1")
            .env("OXIDE_TEST262_HEARTBEAT", &self.hb_path)
            .env("OXIDE_TEST262_PC_WATCH", &self.pc_path)
            .env("OXIDE_TEST262_PATHS_FILE", &self.paths_file)
            .env("OXIDE_TEST262_CHILD_CHUNK", "1")
            .env("OXIDE_TEST262_ALLOW_FAIL_EXIT", "1")
            .env_remove("OXIDE_TEST262_CHUNK_SIZE")
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .map_err(SpawnError)?;
        self.child = Some(child);
        Ok(())
    }

    fn heartbeat(&self) -> Option<Heartbeat> {
        read_heartbeat(&self.hb_path)
    }

    fn merge_fail(&mut self, stats: &mut RunStats) {
        merge_fail_log(stats, &self.hb_path, &mut self.fails_consumed);
    }

    fn scene(&self) -> Option<String> {
        read_pc_scene(&self.pc_path)
    }

    fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
    }

    fn poll(&mut self) -> PollResult {
        let Some(child) = self.child.as_mut() else {
            return PollResult::Alive;
        };
        match child.try_wait() {
            Ok(Some(status)) => PollResult::Exited(status),
            Ok(None) => PollResult::Alive,
            Err(err) => PollResult::WaitError(err),
        }
    }

    fn log_tail(&self) -> Option<String> {
        tail_lines(&self.log_path, 20)
    }

    fn reset_round(&mut self) {
        let _ = std::fs::remove_file(&self.hb_path);
        let _ = std::fs::remove_file(format!("{}.tmp", self.hb_path.display()));
        let _ = std::fs::remove_file(&self.pc_path);
    }

    fn archive_window(&mut self) {
        let _ = std::fs::remove_file(&self.hb_path);
        let _ = std::fs::remove_file(format!("{}.tmp", self.hb_path.display()));
        let _ = std::fs::remove_file(&self.pc_path);
        // 旁路失败行归档而非删除：统计已由心跳合并入父进程，此文件是收尾逐文件
        // 差分的终态依据（删除即永久丢记录）。
        let _ = std::fs::rename(self.hb_path.with_extension("fails"), self.hb_path.with_extension("fails.done"));
        // sidecar 日志同式归档：整窗子进程输出是现场重建的终态依据。
        let _ = std::fs::rename(&self.log_path, self.log_path.with_extension("log.done"));
    }
}

/// 把 supervise 旁路失败行文件并入统计：逐行重建 FailRecord（类别经本表 id
/// 重映射 + push_fail_record 的 cap 逻辑）。
///
/// # 边界与前提
/// - 失败计数（fail_categories）已由对应心跳合并覆盖，此处只补 fail_records，
///   不得再累计计数，避免双计。worker 循环先写心跳后追加旁路，故"无心跳退出"
///   分支旁路必为空（防御性 no-op）。
/// - 旁路文件跨子进程重启追加保留，本函数只解析自上次消费以来的新增段
///   （`consumed` 由传输实例按窗口持有）；文件缩至已消费偏移之下（删除/重建）
///   回绕到文件头重读，避免丢记录。
fn merge_fail_log(stats: &mut RunStats, hb_path: &Path, consumed: &mut usize) {
    let content = match std::fs::read_to_string(hb_path.with_extension("fails")) {
        Ok(c) => c,
        Err(_) => return,
    };
    if content.len() < *consumed {
        *consumed = 0;
    }
    let fresh = &content[*consumed..];
    *consumed = content.len();
    for (index, cat, subkey, message) in parse_fail_log(fresh) {
        stats.push_fail_record(index, cat, subkey, message);
    }
}

/// 读子进程被杀前留下的 last-pc 现场行：取文件末行，须含 pc/op/frames 三字段
/// 才接受（缺文件 / 空文件 / 残行均返回 None）。
fn read_pc_scene(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let last = content.lines().last()?.trim();
    if last.starts_with("pc=") && last.contains(" op=") && last.contains(" frames=") {
        Some(last.to_string())
    } else {
        None
    }
}

/// 读 sidecar 日志的末 `n` 行：文件缺失/空返回 None，有内容时返回整文件或
/// 末 N 行（超时/崩溃后供现场重建）。
fn tail_lines(path: &Path, n: usize) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    if start == lines.len() {
        return None;
    }
    Some(lines[start..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::append_fail_log;

    /// read_pc_scene 防御解析：缺文件 / 空文件 / 残行（缺 frames 字段）均 None，
    /// 末行完整时接受（忽略中间行）。
    #[test]
    fn read_pc_scene_defensive_parse() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_pc_test_{}.txt", std::process::id()));
        assert_eq!(read_pc_scene(&path), None, "缺文件返回 None");
        std::fs::write(&path, "").expect("写失败");
        assert_eq!(read_pc_scene(&path), None, "空文件返回 None");
        std::fs::write(&path, "pc=1 op=ADD flat_id=0 frames=1\npc=2 op=ADD flat_id=0 frames=1\n").expect("写失败");
        assert_eq!(read_pc_scene(&path), Some("pc=2 op=ADD flat_id=0 frames=1".to_string()));
        // 残行：末行缺 frames 字段，返回 None。
        std::fs::write(&path, "pc=1 op=ADD flat_id=0 frames=1\npc=2 op=AD").expect("写失败");
        assert_eq!(read_pc_scene(&path), None, "残行返回 None");
        let _ = std::fs::remove_file(&path);
    }

    /// tail_lines 防御解析：缺文件 / 空文件返回 None，不足 N 行返回整文件，
    /// 超出 N 行只取末 N 行。
    #[test]
    fn tail_lines_defensive_parse() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_log_test_{}.log", std::process::id()));
        assert_eq!(tail_lines(&path, 20), None, "缺文件返回 None");
        std::fs::write(&path, "").expect("写失败");
        assert_eq!(tail_lines(&path, 20), None, "空文件返回 None");
        std::fs::write(&path, "a\nb\n").expect("写失败");
        assert_eq!(tail_lines(&path, 20), Some("a\nb".to_string()), "不足 N 行返回整文件");
        std::fs::write(&path, "1\n2\n3\n4\n").expect("写失败");
        assert_eq!(tail_lines(&path, 2), Some("3\n4".to_string()), "超出 N 行取末 N 行");
        let _ = std::fs::remove_file(&path);
    }

    /// merge_fail_log 重建 fail_records：类别经 id 表重映射、消息 cap；
    /// fail_categories 保持空（计数不双计契约）。
    #[test]
    fn merge_fail_log_reconstructs_fail_records() {
        let dir = std::env::temp_dir();
        let hb_path = dir.join(format!("oxide_t262_hb_test_mrg_{}.txt", std::process::id()));
        let sidecar = hb_path.with_extension("fails");
        append_fail_log(&sidecar, 2, "vm: not callable", "", "x is not callable").expect("追加失败");
        append_fail_log(&sidecar, 7, "compile: unsupported", "", "y").expect("追加失败");
        let mut stats = RunStats::default();
        let mut fails_consumed = 0usize;
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 2);
        // 无新增内容的二次合并不产生重复记录（已消费偏移）。
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 2);
        assert_eq!(stats.fail_records[0].index, 2);
        assert_eq!(stats.categories[stats.fail_records[0].category_id as usize], "vm: not callable");
        assert_eq!(stats.fail_records[0].message, "x is not callable");
        assert_eq!(stats.fail_records[1].index, 7);
        assert_eq!(stats.categories[stats.fail_records[1].category_id as usize], "compile: unsupported");
        assert!(stats.fail_categories.is_empty(), "计数不双计：fail_categories 必须保持空");
        let _ = std::fs::remove_file(&sidecar);
        let _ = std::fs::remove_file(&hb_path);
    }

    /// 跨重启合并序列：旁路文件两次"子进程死亡"间追加保留，第二次合并只并入
    /// 新增段（不重复并入前段行）；重跑测试的新行是真实新事件；文件重建得比
    /// 已消费偏移更短时回绕重读。
    #[test]
    fn merge_fail_log_restart_sequence_no_reread_duplicates() {
        let dir = std::env::temp_dir();
        let hb_path = dir.join(format!("oxide_t262_hb_test_rst_{}.txt", std::process::id()));
        let sidecar = hb_path.with_extension("fails");
        // 段 1：首个子进程死亡前的失败行。
        append_fail_log(&sidecar, 10, "vm: not callable", "", "a").expect("追加失败");
        append_fail_log(&sidecar, 11, "vm: not callable", "", "b").expect("追加失败");
        let mut stats = RunStats::default();
        let mut fails_consumed = 0usize;
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 2);
        // 段 2：第二个子进程追加（重跑的测试 11 至多重录一条 + 新测试 12）。
        append_fail_log(&sidecar, 11, "vm: not callable", "", "b again").expect("追加失败");
        append_fail_log(&sidecar, 12, "compile: unsupported", "", "c").expect("追加失败");
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 4, "只应并入新增段: {:?}", stats.fail_records);
        assert_eq!(stats.fail_records.iter().map(|r| r.index).collect::<Vec<_>>(), vec![10, 11, 11, 12]);
        // 段 3：无新增内容的合并幂等。
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 4);
        // 文件重建得比已消费偏移短：回绕到文件头重读。
        std::fs::write(&sidecar, "20\tvm: not callable\t\td\n").expect("重写失败");
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 5);
        assert_eq!(stats.fail_records[4].index, 20);
        let _ = std::fs::remove_file(&sidecar);
        let _ = std::fs::remove_file(&hb_path);
    }
}
