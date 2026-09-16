//! test262 runner 运行配置：`RunConfig` 结构、CLI 解析（`parse`）与用法输出（`usage`）。
//! 9 字段与 3 关联函数放宽为 `pub(crate)`；`leak_check_interval` 缺省 1000，`usage` 全文即 `--help` 用户可见输出，逐字保留。

use std::path::PathBuf;

/// 运行配置：test262 根目录、路径过滤器及各选项开关。
#[derive(Debug, Default)]
pub(crate) struct RunConfig {
    pub(crate) test262_root: Option<PathBuf>,
    pub(crate) filter: Option<String>,
    pub(crate) no_skip: bool,
    pub(crate) supervise: bool,
    pub(crate) leak_check: bool,
    pub(crate) leak_check_interval: usize,
    /// 关闭 liveness/精确 DCE/RegAlloc 链（on/off 对比基础设施）。
    pub(crate) no_regalloc: bool,
    /// 逐测试打印 PASS/FAIL/SKIP（on/off 结果集合对比用）。
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
            leak_check: false,
            leak_check_interval: 1000,
            no_regalloc: false,
            verbose: false,
            no_fail_list: false,
        }
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
                "--leak-check" => config.leak_check = true,
                "--no-fail-list" => config.no_fail_list = true,
                "--help" | "-h" => return Err(Self::usage()),
                _ if arg.starts_with("--leak-check-interval=") => {
                    config.leak_check_interval =
                        arg.strip_prefix("--leak-check-interval=").unwrap().parse().unwrap_or(1000);
                }
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
        "usage: test262-runner [--no-skip] [--no-regalloc] [--verbose] [--supervise] [--leak-check] [--leak-check-interval=N] [test262-root] [path-filter]\n\
          \n\
          --no-skip    Run capability-excluded tests and count unsupported compile/runtime results as failures.\n\
          --no-regalloc  Disable the liveness/precise-DCE/RegAlloc compiler chain (vregs stay as physical numbers).\n\
          --verbose    Print one PASS/FAIL/SKIP line per test (for on/off result-set comparison).\n\
          --no-fail-list  Do not print the per-path FAIL list at the end of the run.\n\
          --supervise  Run the suite as single-worker child-process windows with a hard per-test timeout and\n\
          \x20            automatic resume past any hanging/crashing test. A hang or crash is reported by path.\n\
          --leak-check Monitor session_object_ptrs, session_bytes, code_forge.len(), symbol_registry.len() every\n\
          \x20            --leak-check-interval tests (default 1000). Flags sustained linear growth (R^2>0.9).\n\
          \n\
          supervised-mode env tunables:\n\
          \x20  OXIDE_TEST262_TIMEOUT_SECS        per-test wall-clock timeout (default 10)\n\
          \x20  OXIDE_TEST262_WINDOW              tests per window (default 5000)\n\
          \x20  OXIDE_TEST262_SUPERVISORS         concurrent windows (default = available parallelism)\n\
          \x20  OXIDE_TEST262_STARTUP_GRACE_SECS  grace for a child's first heartbeat (default 60)"
             .into()
    }
}
