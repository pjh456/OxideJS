//! test262 runner 运行配置：`RunConfig` 结构、CLI 解析（`parse`）与用法输出（`usage`）。
//! 7 字段与 3 关联函数放宽为 `pub(crate)`；`usage` 全文即 `--help` 用户可见输出，逐字保留。

use std::path::PathBuf;

/// 运行配置：test262 根目录、路径过滤器及各选项开关。
#[derive(Debug, Default)]
pub(crate) struct RunConfig {
    pub(crate) test262_root: Option<PathBuf>,
    pub(crate) filter: Option<String>,
    pub(crate) no_skip: bool,
    pub(crate) supervise: bool,
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
            no_regalloc: false,
            verbose: false,
            no_fail_list: false,
        }
    }

    /// 监督模式判定：`--supervise` 且非监督派生的子进程（子进程不重入监督）。
    /// 路径 filter 不参与判定——filter 在子进程内逐测试应用，不构成监督屏障。
    pub(crate) fn supervised(&self, is_chunk_child: bool) -> bool {
        self.supervise && !is_chunk_child
    }

    /// 解析命令行参数为运行配置；未知选项或参数过多返回错误。
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = Self::new();
        let mut positional = Vec::new();

        for arg in args.iter().skip(1) {
            match arg.as_str() {
                "--no-skip" => config.no_skip = true,
                "--no-regalloc" => config.no_regalloc = true,
                "--verbose" => config.verbose = true,
                "--supervise" => config.supervise = true,
                "--no-fail-list" => config.no_fail_list = true,
                "--help" | "-h" => return Err(Self::usage()),
                _ if arg.starts_with("--") => return Err(format!("unknown option: {arg}\n\n{}", Self::usage())),
                _ => positional.push(arg.clone()),
            }
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
        "usage: test262-runner [--no-skip] [--no-regalloc] [--verbose] [--supervise] [--version] [test262-root] [path-filter]\n\
          \n\
          --no-skip    Run capability-excluded tests and count unsupported compile/runtime results as failures.\n\
          --no-regalloc  Disable the liveness/precise-DCE/RegAlloc compiler chain (vregs stay as physical numbers).\n\
          --verbose    Print one PASS/FAIL/SKIP line per test (for on/off result-set comparison).\n\
          --no-fail-list  Do not print the per-path FAIL list at the end of the run.\n\
          --version    Print the binary version with the build-time git short hash (freshness self-check), then exit.\n\
          --supervise  Run the suite as single-worker child-process windows with a hard per-test timeout and\n\
          \x20            automatic resume past any hanging/crashing test. A hang or crash is reported by path.\n\
          \x20            Combines with [path-filter]: the filter is applied per-test inside each child window.\n\
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
          \x20  OXIDE_TEST262_KERNEL_BATCH           kernel rebuilds every N tests (default 5000, 1000 under --no-skip)\n\
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
