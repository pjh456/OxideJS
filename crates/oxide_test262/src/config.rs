//! test262 runner 运行配置：`RunConfig` 结构、CLI 解析（`parse`）与用法输出（`usage`）。
//! 8 字段与 3 关联函数放宽为 `pub(crate)`；`usage` 全文即 `--help` 用户可见输出，逐字保留。

use std::net::SocketAddr;
use std::path::PathBuf;

/// 运行配置：test262 根目录、路径过滤器及各选项开关。
#[derive(Debug, Default)]
pub(crate) struct RunConfig {
    pub(crate) test262_root: Option<PathBuf>,
    pub(crate) filter: Option<String>,
    pub(crate) no_skip: bool,
    pub(crate) supervise: bool,
    /// 远程分片 worker 模式：连监督者端点收窗口指派帧，in-process 执行并发帧。
    pub(crate) worker: bool,
    /// 远程监督模式：（端点文本, worker 数）。监督者 bind 端点、每窗口 accept 一条
    /// worker 连接，经 Tcp 传输执行窗口。
    pub(crate) remote_supervise: Option<(String, usize)>,
    /// safe 模式：并行 worker 从线程变进程派生，IPC 走 Unix socket 复用分片帧协议。
    /// 缺省 off（unsafe）保现状逐位不变；与 --supervise/--worker/--remote-supervise 互斥。
    pub(crate) safe: bool,
    /// 关闭 liveness/精确 DCE/RegAlloc 链（开关对比：关优化链 vs 开优化链各跑一遍）。
    pub(crate) no_regalloc: bool,
    /// 逐测试打印 PASS/FAIL/SKIP（开关对比时对比两轮结果集合用）。
    pub(crate) verbose: bool,
    /// 汇总尾部不打印 FAIL 清单。
    pub(crate) no_fail_list: bool,
}

impl RunConfig {
    /// 默认运行配置。
    pub(crate) fn new() -> Self {
        Self {
            test262_root: None,
            filter: None,
            no_skip: false,
            supervise: false,
            worker: false,
            remote_supervise: None,
            safe: false,
            no_regalloc: false,
            verbose: false,
            no_fail_list: false,
        }
    }

    /// 监督模式判定：`--supervise` 或 `--remote-supervise` 且非监督派生的子进程
    /// （子进程不重入监督）。路径 filter 不参与判定——filter 在子进程内逐测试
    /// 应用，不构成监督屏障。
    pub(crate) fn supervised(&self, is_chunk_child: bool) -> bool {
        (self.supervise || self.remote_supervise.is_some()) && !is_chunk_child
    }

    /// 解析 `--remote-supervise` 端点：缺省补 loopback、解析期拒绝非 loopback。
    ///
    /// # 步骤
    /// 1. `:port` 形态补 `127.0.0.1` 前缀，裸端口（无冒号）补 `127.0.0.1:` 前缀。
    /// 2. 解析为 `SocketAddr`；失败（含主机名形态）返回错误。
    /// 3. 非 loopback 地址（0.0.0.0、局域网 IP）返回错误。
    ///
    /// # 边界与前提
    /// - 端点只接受 `[IP]:port` 形态；主机名不解析（无法在解析期验证 loopback）。
    /// - 比 server 的「0.0.0.0 可配置但不受支持」更严：worker 执行不可信测试
    ///   内容，非 loopback 绑定会把窗口指派面（含本地路径执行）暴露到网络。
    ///
    /// # 副作用
    /// - 无（纯函数，返回归一化后的端点文本）。
    fn parse_remote_supervise_endpoint(raw: &str) -> Result<String, String> {
        let normalized = if raw.starts_with(':') {
            format!("127.0.0.1{raw}")
        } else if !raw.contains(':') {
            format!("127.0.0.1:{raw}")
        } else {
            raw.to_string()
        };
        let addr: SocketAddr = normalized
            .parse()
            .map_err(|_| format!("invalid remote-supervise endpoint: {raw} (expected [IP]:port, loopback only)"))?;
        if !addr.ip().is_loopback() {
            return Err(format!(
                "remote-supervise endpoint must bind to loopback only: {raw} (workers execute untrusted test content; non-loopback binding exposes the window assignment surface to the network)"
            ));
        }
        Ok(normalized)
    }

    /// 解析命令行参数为运行配置；未知选项、参数过多或 `--remote-supervise`
    /// 缺实参 / 端点非 loopback / worker 数非法返回错误。
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = Self::new();
        let mut positional = Vec::new();
        let mut i = 1;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "--no-skip" => config.no_skip = true,
                "--no-regalloc" => config.no_regalloc = true,
                "--verbose" => config.verbose = true,
                "--supervise" => config.supervise = true,
                "--worker" => config.worker = true,
                "--safe" => config.safe = true,
                "--no-fail-list" => config.no_fail_list = true,
                // --remote-supervise 消费后两个实参：端点文本与 worker 数。
                // 端点解析期归一化：缺省补 loopback，非 loopback 拒绝。
                "--remote-supervise" => {
                    let endpoint_raw = args
                        .get(i + 1)
                        .ok_or_else(|| format!("--remote-supervise requires an endpoint\n\n{}", Self::usage()))?;
                    let endpoint = Self::parse_remote_supervise_endpoint(endpoint_raw)
                        .map_err(|e| format!("{e}\n\n{}", Self::usage()))?;
                    let workers_str = args
                        .get(i + 2)
                        .ok_or_else(|| format!("--remote-supervise requires a worker count\n\n{}", Self::usage()))?;
                    let workers = workers_str
                        .parse::<usize>()
                        .map_err(|_| format!("invalid worker count: {workers_str}\n\n{}", Self::usage()))?;
                    config.remote_supervise = Some((endpoint, workers));
                    i += 2;
                }
                "--help" | "-h" => return Err(Self::usage()),
                _ if arg.starts_with("--") => return Err(format!("unknown option: {arg}\n\n{}", Self::usage())),
                _ => positional.push(arg.clone()),
            }
            i += 1;
        }

        // worker 与 supervise 互斥：前者是远程分片的执行半区（连监督者端点），
        // 后者是本地子进程监督，两者不共存。
        if config.worker && config.supervise {
            return Err(format!("--worker and --supervise are mutually exclusive\n\n{}", Self::usage()));
        }
        // remote-supervise 是远程监督半区，与 worker（执行半区）及本地 supervise 互斥。
        if config.worker && config.remote_supervise.is_some() {
            return Err(format!("--worker and --remote-supervise are mutually exclusive\n\n{}", Self::usage()));
        }
        if config.supervise && config.remote_supervise.is_some() {
            return Err(format!("--supervise and --remote-supervise are mutually exclusive\n\n{}", Self::usage()));
        }
        // safe 模式是本地远程监督的 Unix socket 变体（自动派生 worker 进程），
        // 与已各自进程化的 --supervise / --remote-supervise 及执行半区 --worker 互斥。
        if config.safe && config.supervise {
            return Err(format!("--safe and --supervise are mutually exclusive\n\n{}", Self::usage()));
        }
        if config.safe && config.worker {
            return Err(format!("--safe and --worker are mutually exclusive\n\n{}", Self::usage()));
        }
        if config.safe && config.remote_supervise.is_some() {
            return Err(format!("--safe and --remote-supervise are mutually exclusive\n\n{}", Self::usage()));
        }

        if let Some(root) = positional.first() {
            config.test262_root = Some(PathBuf::from(root));
        }
        if let Some(filter) = positional.get(1) {
            config.filter = Some(filter.clone());
        }
        if positional.len() > 2 {
            return Err(format!("too many positional arguments\n\n{}", Self::usage()));
        }

        Ok(config)
    }

    /// 打印用法说明。
    pub(crate) fn usage() -> String {
        "usage: test262-runner [--no-skip] [--no-regalloc] [--verbose] [--supervise] [--worker] [--remote-supervise <endpoint> <workers>] [--version] [test262-root] [path-filter]\n\
          \n\
          --no-skip    Run capability-excluded tests and count unsupported compile/runtime results as failures.\n\
          --no-regalloc  Disable the liveness/precise-DCE/RegAlloc compiler chain (vregs stay as physical numbers).\n\
          --verbose    Print one PASS/FAIL/SKIP line per test; SKIP lines carry the skip reason (for on/off result-set comparison).\n\
          --no-fail-list  Do not print the per-path FAIL list at the end of the run.\n\
          --version    Print the binary version with the build-time git short hash (freshness self-check), then exit.\n\
          --supervise  Run the suite as single-worker child-process windows with a hard per-test timeout and\n\
          \x20            automatic resume past any hanging/crashing test. A hang or crash is reported by path.\n\
          \x20            Combines with [path-filter]: the filter is applied per-test inside each child window.\n\
          --worker     Run as a remote shard worker: connect to the supervisor endpoint and execute assigned\n\
          \x20            test windows in-process, sending heartbeat/fail/scene/DONE frames over the connection.\n\
          \x20            Authenticates with the token from OXIDE_TEST262_WORKER_TOKEN (out-of-band from the\n\
          \x20            supervisor's foreground print). Mutually exclusive with --supervise.\n\
          --remote-supervise  Run as a remote supervisor: bind <endpoint> (loopback [IP]:port; a bare port\n\
          \x20            defaults to 127.0.0.1, non-loopback addresses are rejected) and execute test\n\
          \x20            windows on <workers> remote workers, sending assign frames and receiving\n\
          \x20            heartbeat/fail/scene/DONE frames over the accepted connections. A worker\n\
          \x20            disconnect is recovered as a child-process death. Mutually exclusive with\n\
          \x20            --supervise and --worker.\n\
          --safe       Run the parallel workers as spawned processes (crash isolation) with IPC over a\n\
          \x20            local Unix socket reusing the shard frame protocol. A worker process is respawned\n\
          \x20            after a timeout/crash and resumes past the culprit. Off by default (unsafe thread\n\
          \x20            path is unchanged). Mutually exclusive with --supervise, --worker and\n\
          \x20            --remote-supervise.\n\
          \n\
          env tunables (all optional):\n\
          \x20  OXIDE_LOG                          per-subsystem log levels (default off)\n\
          \x20  OXIDE_TEST262_WORKERS              worker threads in parallel mode (default = available parallelism, 4 under --no-skip)\n\
          \x20  OXIDE_TEST262_WINDOW                 supervised tests per child window (default 5000)\n\
          \x20  OXIDE_TEST262_TIMEOUT_SECS          per-test wall-clock timeout in supervised mode (default 10)\n\
          \x20  OXIDE_TEST262_STARTUP_GRACE_SECS    grace for a child's first heartbeat (default 60)\n\
          \x20  OXIDE_TEST262_SUPERVISORS          concurrent supervised windows (default 16)\n\
          \x20  OXIDE_TEST262_RUNNING_LOG            log every test as it starts (any value)\n\
          \x20  OXIDE_TEST262_HEARTBEAT              heartbeat file path (supervised/chunked)\n\
          \x20  OXIDE_TEST262_PC_WATCH              last-pc scene file path (supervised child appends pc/opcode/frames every 65536 instructions)\n\
          \x20  OXIDE_TEST262_CHUNK_SIZE             tests per child in chunked mode\n\
          \x20  OXIDE_TEST262_CHILD_CHUNK            marks a spawned child as chunk worker (any value)\n\
          \x20  OXIDE_TEST262_ALLOW_FAIL_EXIT        child may exit 1 on fail (any value)\n\
          \x20  OXIDE_TEST262_WORKER_ENDPOINT       supervisor endpoint for --worker mode (host:port)\n\
          \x20  OXIDE_TEST262_WORKER_TOKEN          worker auth token for --worker mode (out-of-band from the supervisor's foreground print)\n\
          \x20  OXIDE_TEST262_KERNEL_BATCH           kernel rebuilds every N tests (default 256, clamped to the 512 realm bound)\n\
          \x20  OXIDE_TEST262_LOG_LEVEL              runner log level (default info)\n\
          \x20  OXIDE_SKIP_UNTIL                      skip test indexes below N (chunking/supervise resume)\n\
          \x20  OXIDE_MAX_TESTS                       run at most N tests after skip point"
             .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造解析用参数字列（程序名 + 实参）。
    fn args(args: &[&str]) -> Vec<String> {
        let mut v = vec!["test262-runner".to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        v
    }

    /// 五旗标 + 2 位置参数一次 parse：五裸旗各置位、
    /// 位置参数落 root/filter，Ok 路径全断言。
    #[test]
    fn parse_all_flags_with_two_positionals() {
        let cfg = RunConfig::parse(&args(&[
            "--no-skip",
            "--no-regalloc",
            "--verbose",
            "--supervise",
            "--no-fail-list",
            "tests/test262",
            "language",
        ]))
        .expect("full-flag parse should succeed");
        assert!(cfg.no_skip);
        assert!(cfg.no_regalloc);
        assert!(cfg.verbose);
        assert!(cfg.supervise);
        assert!(cfg.no_fail_list);
        assert_eq!(cfg.test262_root, Some(PathBuf::from("tests/test262")));
        assert_eq!(cfg.filter.as_deref(), Some("language"));
    }

    /// --worker 旗标置位；与 --supervise 同时给返回互斥错误。
    #[test]
    fn parse_worker_flag_and_supervise_exclusive() {
        let cfg = RunConfig::parse(&args(&["--worker"])).expect("worker flag should parse");
        assert!(cfg.worker, "worker 旗标应置位");
        assert!(!cfg.supervise, "supervise 旗标不应置位");

        let err =
            RunConfig::parse(&args(&["--worker", "--supervise"])).expect_err("worker+supervise must be exclusive");
        assert!(err.contains("mutually exclusive"), "应提示互斥：{err}");
    }

    /// --remote-supervise 正常解析端点与 worker 数；缺实参、worker 数非法返回错误。
    #[test]
    fn parse_remote_supervise_flag() {
        let cfg = RunConfig::parse(&args(&["--remote-supervise", "127.0.0.1:8080", "4"]))
            .expect("remote-supervise 应解析成功");
        assert_eq!(cfg.remote_supervise, Some(("127.0.0.1:8080".to_string(), 4)));
        assert!(!cfg.supervise, "remote-supervise 不应置位 supervise");
        assert!(!cfg.worker, "remote-supervise 不应置位 worker");

        let err = RunConfig::parse(&args(&["--remote-supervise"])).expect_err("缺端点应报错");
        assert!(err.contains("--remote-supervise requires an endpoint"), "应提示缺端点：{err}");

        let err = RunConfig::parse(&args(&["--remote-supervise", "127.0.0.1:8080"])).expect_err("缺 worker 数应报错");
        assert!(err.contains("--remote-supervise requires a worker count"), "应提示缺 worker 数：{err}");

        let err =
            RunConfig::parse(&args(&["--remote-supervise", "127.0.0.1:8080", "abc"])).expect_err("worker 数非法应报错");
        assert!(err.contains("invalid worker count"), "应提示 worker 数非法：{err}");
    }

    /// --remote-supervise 端点缺省补 loopback：裸端口与 `:port` 形态归一化为
    /// 127.0.0.1 前缀，显式 loopback 地址原样保留。
    #[test]
    fn parse_remote_supervise_endpoint_defaults_loopback() {
        let cfg = RunConfig::parse(&args(&["--remote-supervise", ":4243", "2"])).expect(":port 形态应补 loopback");
        assert_eq!(cfg.remote_supervise, Some(("127.0.0.1:4243".to_string(), 2)));

        let cfg = RunConfig::parse(&args(&["--remote-supervise", "4243", "2"])).expect("裸端口应补 loopback");
        assert_eq!(cfg.remote_supervise, Some(("127.0.0.1:4243".to_string(), 2)));

        let cfg =
            RunConfig::parse(&args(&["--remote-supervise", "127.0.0.1:4243", "2"])).expect("显式 loopback 应保留");
        assert_eq!(cfg.remote_supervise, Some(("127.0.0.1:4243".to_string(), 2)));
    }

    /// --remote-supervise 端点解析期拒绝非 loopback（0.0.0.0、局域网 IP）与
    /// 非法形态（主机名、非端口文本）。
    #[test]
    fn parse_remote_supervise_endpoint_rejects_non_loopback() {
        for raw in ["0.0.0.0:4243", "192.168.1.5:4243", "localhost:4243", "notanendpoint"] {
            let err = RunConfig::parse(&args(&["--remote-supervise", raw, "2"])).expect_err("应拒绝：{raw}");
            assert!(
                err.contains("loopback") || err.contains("invalid remote-supervise endpoint"),
                "拒绝文案应说明 loopback 口径或端点非法：{err}"
            );
        }
    }

    /// --remote-supervise 与 --worker、--supervise 互斥。
    #[test]
    fn parse_remote_supervise_exclusive() {
        let err = RunConfig::parse(&args(&["--remote-supervise", "127.0.0.1:8080", "4", "--worker"]))
            .expect_err("worker+remote-supervise 应互斥");
        assert!(err.contains("mutually exclusive"), "应提示互斥：{err}");

        let err = RunConfig::parse(&args(&["--remote-supervise", "127.0.0.1:8080", "4", "--supervise"]))
            .expect_err("supervise+remote-supervise 应互斥");
        assert!(err.contains("mutually exclusive"), "应提示互斥：{err}");
    }

    /// --safe 旗标置位；与 --supervise / --worker / --remote-supervise 两两互斥；
    /// safe 不触发 supervised()（独立执行路径，非监督模式）。
    #[test]
    fn parse_safe_flag_and_exclusive() {
        let cfg = RunConfig::parse(&args(&["--safe"])).expect("safe 旗标应解析");
        assert!(cfg.safe, "safe 旗标应置位");
        assert!(!cfg.supervise, "safe 不应置位 supervise");
        assert!(!cfg.supervised(false), "safe 不应触发 supervised");

        let err = RunConfig::parse(&args(&["--safe", "--supervise"])).expect_err("safe+supervise 应互斥");
        assert!(err.contains("mutually exclusive"), "应提示互斥：{err}");

        let err = RunConfig::parse(&args(&["--safe", "--worker"])).expect_err("safe+worker 应互斥");
        assert!(err.contains("mutually exclusive"), "应提示互斥：{err}");

        let err = RunConfig::parse(&args(&["--safe", "--remote-supervise", "127.0.0.1:8080", "4"]))
            .expect_err("safe+remote 应互斥");
        assert!(err.contains("mutually exclusive"), "应提示互斥：{err}");
    }

    /// remote_supervise 置位也触发 supervised()（非子进程）；子进程不重入。
    #[test]
    fn supervised_predicate_with_remote_supervise() {
        let mut cfg = RunConfig::new();
        cfg.remote_supervise = Some(("127.0.0.1:8080".to_string(), 4));
        assert!(cfg.supervised(false), "remote_supervise 应触发 supervised");
        assert!(!cfg.supervised(true), "子进程不应重入 supervised");
    }

    /// 未知选项返回含 "unknown option" 的错误；--help 的错误文案与 usage 逐字相等。
    #[test]
    fn parse_unknown_option_and_help_err_text() {
        let err = RunConfig::parse(&args(&["--bogus"])).expect_err("unknown option must error");
        assert!(err.contains("unknown option: --bogus"));

        let help = RunConfig::parse(&args(&["--help"])).expect_err("--help must error");
        assert_eq!(help, RunConfig::usage());
    }

    /// 监督判定三态：开启时 filter 不否决、子进程不重入、未开启恒 false。
    #[test]
    fn supervised_predicate_ignores_filter() {
        let mut cfg = RunConfig::new();
        cfg.supervise = true;
        cfg.filter = Some("language".to_string());
        assert!(cfg.supervised(false), "filter 不得否决监督");
        assert!(!cfg.supervised(true), "子进程不得重入监督");
        let off = RunConfig::new();
        assert!(!off.supervised(false));
    }

    /// leak-check 旗标不在已知选项集内，落未知选项分支返回错误（回归钉）；
    /// 位置参数超过 2 个返回错误。
    #[test]
    fn parse_leak_check_flags_unknown_and_positional_limit() {
        let err = RunConfig::parse(&args(&["--leak-check"])).expect_err("removed flag must be rejected");
        assert!(err.contains("unknown option: --leak-check"));

        let err = RunConfig::parse(&args(&["--leak-check-interval=abc"]))
            .expect_err("removed interval flag must be rejected");
        assert!(err.contains("unknown option: --leak-check-interval=abc"));

        let err = RunConfig::parse(&args(&["a", "b", "c"])).expect_err("three positionals must error");
        assert!(err.contains("too many positional arguments"));
    }
}
