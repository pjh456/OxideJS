#![allow(clippy::arc_with_non_send_sync)]
#![allow(dead_code)]

use std::fs;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ansi_term::Colour::Red;
use clap::{Parser, Subcommand};
use oxide_cli::format_js_value;
use oxide_cli::server::cleanup::{well_known_cleanup, CleanupOutcome};
use oxide_cli::server::server::{run_server, run_server_rm, RmServerConfig, ServerConfig};
use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_compiler::compiler_error;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_kernel::shape_forge::ShapeForge;
use oxide_kernel::string_forge::PermInterner;
use oxide_kernel::{kernel_error, kernel_info};
use oxide_log::{Level, SUBSYSTEM_COUNT};
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;
use oxide_vm::vm_error;
use oxide_vm::vm_pool::VmPool;
use oxide_vm::JsValue;

mod bench;

#[derive(Parser)]
#[command(
    name = "oxide",
    // 版本号后附构建期 git 短哈希，`--version` 自报即新鲜度自证锚点。
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_COMMIT"), ")"),
    about = "OxideJS - Rust JavaScript engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, global = true)]
    verbose: bool,

    #[arg(short, long, global = true)]
    quiet: bool,

    /// 聚合并输出现有计数器（GC 统计、内联缓存、session 堆账目、指令数）。
    /// 默认关闭，关闭时零开销；输出每指标一行，与 bench 输出对齐。
    #[arg(long, global = true)]
    profile: bool,
}

#[derive(Subcommand)]
enum Commands {
    Eval {
        code: String,
        /// 逐指令 trace：每条指令向 stderr 写一行 pc + opcode + 操作数。
        #[arg(long)]
        trace: bool,
    },
    Run {
        file: String,
        #[arg(long, default_value = "1")]
        repeat: u64,
        /// 逐指令 trace：每条指令向 stderr 写一行 pc + opcode + 操作数。
        #[arg(long)]
        trace: bool,
    },
    Compile {
        #[arg(short = 'e')]
        expr: Option<String>,
        file: Option<String>,
        #[arg(long)]
        no_dce: bool,
        /// 关闭 liveness/精确 DCE/RegAlloc 链，vreg 原样当物理号（调试降级路径）。
        #[arg(long)]
        no_regalloc: bool,
    },
    Bench {
        #[arg(default_value = "js")]
        mode: Option<String>,
        #[arg(short, long)]
        filter: Option<String>,
        #[arg(long, default_value = "2")]
        warmup: u32,
        #[arg(long, default_value = "10")]
        iterations: u32,
        #[arg(long)]
        update_baseline: bool,
        /// 指令周期采样周期（2 的幂，0 关闭，默认关闭）。开启时测量迭代每
        /// period 条指令记一条样本，run 末按 flat_id 输出 top-K 直方图。
        #[arg(long, default_value = "0")]
        sample: u64,
        /// 采样直方图 top-K 大小（默认 10）。
        #[arg(long, default_value = "10")]
        sample_top: usize,
    },
    Test {
        suite: Option<String>,
    },
    /// 持久 server：常驻进程，复用预热池，近零 spawn 成本。
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
}

/// server 子命令：持久 server 进程管理与 --rm 独立模式。
#[derive(Subcommand)]
enum ServerCommands {
    /// 启动 server。持久形态前台常驻（阻塞至关闭请求、信号或 yield 触发退出）；
    /// --rm 为独立形态（进程唯一 socket、不注册 sidecar、空闲超时或断开即退出）。
    Start {
        /// 独立模式：进程唯一 socket 路径，不注册 sidecar，空闲超时或断开即自动退出。
        #[arg(long)]
        rm: bool,
        /// 空闲超时（秒），仅对 --rm 有效；窗口内无连接或连接上无数据即自动退出。
        #[arg(long, default_value = "30")]
        idle_timeout: u64,
        /// 常驻 worker 线程数；缺省取宿主核数。
        #[arg(long)]
        workers: Option<u32>,
    },
    /// 向持久 server 发送关闭请求。
    Stop,
    /// 查询 server 状态（池与运行时长）。
    Status,
    /// 健康检查。
    Health,
    /// 查询 server 信息（版本、socket 路径、进程号）。
    Info,
    /// 查询 server 版本。
    Version,
    /// 清理残留文件（无 sidecar、陈旧 socket、杀刻度匹配进程）。
    Cleanup,
    /// 向持久 server 发送 yield 请求（排空退出后重新接管 socket 路径）。
    Restart,
    /// 查看 server 日志。
    Log,
    /// 查询内部状态（代码、对象、字符串、属性）。
    Forge,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Eval { code, trace }) => {
            let kernel = make_kernel(cli.verbose, cli.quiet);
            let pool = make_pool(&kernel);
            eval(&code, &kernel, &pool, trace, cli.profile, true)
        }
        Some(Commands::Run { file, repeat, trace }) => {
            let kernel = make_kernel(cli.verbose, cli.quiet);
            let pool = make_pool(&kernel);
            for n in 0..repeat {
                let code = run(&file, &kernel, &pool, trace, cli.profile);
                // 失败即终止并传播退出码，供脚本与 CI 区分成败。
                if code != ExitCode::SUCCESS {
                    return code;
                }
                if n + 1 < repeat {
                    eprintln!("[oxide] iteration {}/{} done", n + 1, repeat);
                }
            }
            ExitCode::SUCCESS
        }
        Some(Commands::Compile {
            expr,
            file,
            no_dce,
            no_regalloc,
        }) => compile(expr, file, no_dce, no_regalloc),
        Some(Commands::Bench {
            mode,
            filter,
            warmup,
            iterations,
            update_baseline,
            sample,
            sample_top,
        }) => {
            let kernel = make_kernel(false, false);
            let pool = make_pool(&kernel);
            // 采样周期边界检查：须为 0（关闭）或 2 的幂，否则指令边界的
            // `steps & (period - 1) == 0` 判定退化（周期不整除步数序列）。
            if sample != 0 && sample & (sample - 1) != 0 {
                eprintln!("--sample period must be 0 (disabled) or a power of two, got {sample}");
                return ExitCode::FAILURE;
            }
            let config = bench::BenchConfig {
                mode: mode.unwrap_or_else(|| "js".to_string()),
                filter,
                warmup,
                iterations,
                update_baseline,
                sample_period: sample,
                sample_top,
            };
            bench::run_benchmarks(config, kernel, pool)
        }
        Some(Commands::Test { .. }) => not_implemented("test"),
        Some(Commands::Server { command }) => server_command(command),
        None => repl(),
    }
}

/// server 子命令分派：start（两形态）与 cleanup 为最小实现，其余八臂为
/// not_implemented 占位。
///
/// # 步骤
/// 1. Start：rm 为假调持久 server 入口（well-known 路径），rm 为真调独立
///    模式入口（进程唯一路径）；`--workers` 为 Some 时覆盖 worker 数。
/// 2. Cleanup：调 well-known 路径清理入口，四态结果映射退出码（无残留与
///    已清理退 0，拒绝与杀进程失败退 1）并打印结果。
/// 3. 其余八臂：not_implemented 占位（退出码 2）。
///
/// # 边界与前提
/// - `--idle-timeout` 不带 --rm 时静默忽略（语义门控归后续任务）。
/// - 全局旗标 -v / -q / --profile 在 server 臂为空操作。
///
/// # 副作用
/// - start 阻塞当前进程至 server 退出（持久形态为前台常驻）。
/// - cleanup 可能向 sidecar 记录的进程发 SIGTERM 并删除残留文件。
fn server_command(command: ServerCommands) -> ExitCode {
    match command {
        ServerCommands::Start { rm, idle_timeout, workers } => {
            if rm {
                let mut config = RmServerConfig {
                    idle_timeout: Duration::from_secs(idle_timeout),
                    ..RmServerConfig::default()
                };
                if let Some(n) = workers {
                    config.worker_count = n as usize;
                }
                start_result(run_server_rm(&config))
            } else {
                let mut config = ServerConfig::well_known();
                if let Some(n) = workers {
                    config.worker_count = n as usize;
                }
                start_result(run_server(&config))
            }
        }
        ServerCommands::Cleanup => match well_known_cleanup() {
            CleanupOutcome::Clean => {
                println!("无残留：sidecar 与 socket 文件均不存在。");
                ExitCode::SUCCESS
            }
            CleanupOutcome::Removed => {
                println!("残留已清理：孤儿文件已删除。");
                ExitCode::SUCCESS
            }
            CleanupOutcome::RefusedLiveServer => {
                eprintln!("存在存活 server 且进程号无法可靠确定，未触碰文件，需人工检查。");
                ExitCode::FAILURE
            }
            CleanupOutcome::KillFailed => {
                eprintln!("已发终止信号但进程在等待窗口内未退出，未触碰文件。");
                ExitCode::FAILURE
            }
        },
        ServerCommands::Stop => not_implemented("server stop"),
        ServerCommands::Status => not_implemented("server status"),
        ServerCommands::Health => not_implemented("server health"),
        ServerCommands::Info => not_implemented("server info"),
        ServerCommands::Version => not_implemented("server version"),
        ServerCommands::Restart => not_implemented("server restart"),
        ServerCommands::Log => not_implemented("server log"),
        ServerCommands::Forge => not_implemented("server forge"),
    }
}

/// 启动结果到退出码：成功退 0，错误以红色打印到 stderr 后退 1。
fn start_result(result: Result<(), oxide_cli::server::server::ServerError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{}", Red.paint(format!("server 启动失败：{err}")));
            ExitCode::FAILURE
        }
    }
}

fn make_kernel(verbose: bool, quiet: bool) -> Arc<KernelCore> {
    let mut config = KernelConfig::standard();
    if verbose {
        config.log_levels = [Level::Info; SUBSYSTEM_COUNT];
    } else if quiet {
        config.log_levels = [Level::Off; SUBSYSTEM_COUNT];
    } else {
        config.log_levels = [Level::Error; SUBSYSTEM_COUNT];
    }
    KernelCore::new(config)
}

fn make_pool(kernel: &Arc<KernelCore>) -> Arc<VmPool> {
    VmPool::new(Arc::clone(kernel), kernel.config.min_pool_size, kernel.config.max_pool_size)
}

fn eval(
    code: &str, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>, trace: bool, profile: bool, print_result: bool,
) -> ExitCode {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, code) {
        Ok(p) => p,
        Err(errors) => {
            for err in &errors {
                compiler_error!("parse error: {}", err);
                eprintln!("{}", Red.paint(err.to_string()));
            }
            return ExitCode::FAILURE;
        }
    };

    let compiler = Compiler::new();
    let hash = compiled_module_hash(&program);
    let module = match kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program)) {
        Ok(m) => m,
        Err(err) => {
            compiler_error!("compile error: {}", err);
            eprintln!("{}", Red.paint(err));
            return ExitCode::FAILURE;
        }
    };

    let mut guard = pool.spawn();
    guard.vm_mut().set_instruction_trace(trace);
    let exec_start = Instant::now();
    match guard.vm_mut().run(&module) {
        Ok(result) => {
            // --profile 开启时聚合现有计数器，每指标一行输出到 stderr（stdout 保持纯结果）。
            if profile {
                let p = bench::profile::ProfileOutput::collect(guard.vm(), exec_start.elapsed().as_micros() as u64);
                for (name, value) in p.iter_metrics() {
                    eprintln!("{name} = {value}");
                }
            }
            // eval 臂按 REPL 语义打印完成值；run 臂脚本只输出自身产生内容。
            if print_result {
                format_result(guard.vm(), kernel.perm_interner().as_ref(), kernel.shape_forge().as_ref(), result);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            vm_error!("runtime error: {}", err);
            eprintln!("{}", Red.paint(err));
            ExitCode::FAILURE
        }
    }
}

fn format_result(vm: &oxide_vm::vm::Vm, string_forge: &PermInterner, shape_forge: &ShapeForge, val: JsValue) {
    println!("{}", format_js_value(vm, string_forge, shape_forge, val));
}

fn run(file: &str, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>, trace: bool, profile: bool) -> ExitCode {
    match fs::read_to_string(file) {
        Ok(source) => eval(&source, kernel, pool, trace, profile, false),
        Err(err) => {
            kernel_error!("cannot read {}: {}", file, err);
            eprintln!("{}", Red.paint(format!("Cannot read {file}: {err}")));
            ExitCode::FAILURE
        }
    }
}

fn compile(expr: Option<String>, file: Option<String>, no_dce: bool, no_regalloc: bool) -> ExitCode {
    let source = if let Some(code) = expr {
        code
    } else if let Some(path) = file {
        match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(err) => {
                kernel_error!("cannot read {}: {}", path, err);
                eprintln!("{}", Red.paint(format!("Cannot read {path}: {err}")));
                return ExitCode::FAILURE;
            }
        }
    } else {
        kernel_error!("compile requires -e '<code>' or a file path");
        eprintln!("{}", Red.paint("compile requires -e '<code>' or a file path"));
        return ExitCode::FAILURE;
    };

    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, &source) {
        Ok(p) => p,
        Err(errors) => {
            for err in &errors {
                compiler_error!("parse error: {}", err);
                eprintln!("{}", Red.paint(err.to_string()));
            }
            return ExitCode::FAILURE;
        }
    };

    let mut compiler = Compiler::new();
    if no_dce {
        compiler = compiler.with_dce(false);
    }
    if no_regalloc {
        compiler = compiler.with_regalloc(false);
    }
    match compiler.compile(&program) {
        Ok(module) => {
            print!("{module}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            compiler_error!("compile error: {}", err);
            eprintln!("{}", Red.paint(err));
            ExitCode::FAILURE
        }
    }
}

fn bracket_balance(line: &str) -> i32 {
    let mut count = 0i32;
    for ch in line.chars() {
        match ch {
            '(' | '[' | '{' => count += 1,
            ')' | ']' | '}' => count -= 1,
            _ => {}
        }
    }
    count
}

fn repl() -> ExitCode {
    use rustyline::error::ReadlineError;
    use rustyline::DefaultEditor;

    let mut rl = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(err) => {
            kernel_error!("failed to start REPL: {}", err);
            eprintln!("{}", Red.paint(format!("Failed to start REPL: {err}")));
            return ExitCode::FAILURE;
        }
    };

    let kernel = make_kernel(false, false);
    let mut vm = Vm::with_kernel_core(Arc::clone(&kernel));
    let mut input_buf = String::new();

    loop {
        let prompt = if input_buf.is_empty() { "oxide> " } else { "...> " };
        match rl.readline(prompt) {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == ".exit" || trimmed == ".quit" {
                    println!("exit");
                    return ExitCode::SUCCESS;
                }
                rl.add_history_entry(&trimmed).ok();

                if !input_buf.is_empty() {
                    input_buf.push('\n');
                }
                input_buf.push_str(&trimmed);

                let balance = bracket_balance(&input_buf);
                if balance > 0 {
                    continue;
                }

                let result = eval_repl(&input_buf, &kernel, &mut vm);
                input_buf.clear();

                if result == ExitCode::FAILURE {
                    // eval_repl 已打印错误，此处仅保持缓冲区已清空，避免重复输出。
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                return ExitCode::SUCCESS;
            }
            Err(ReadlineError::Eof) => {
                println!("exit");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                vm_error!("REPL error: {}", err);
                eprintln!("{}", Red.paint(format!("REPL error: {err}")));
                return ExitCode::FAILURE;
            }
        }
    }
}

fn eval_repl(code: &str, kernel: &Arc<KernelCore>, vm: &mut Vm) -> ExitCode {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, code) {
        Ok(p) => p,
        Err(errors) => {
            for err in &errors {
                compiler_error!("parse error: {}", err);
                eprintln!("{}", Red.paint(err.to_string()));
            }
            return ExitCode::FAILURE;
        }
    };

    let compiler = Compiler::new().with_repl_persist(true);
    let hash = compiled_module_hash(&program);
    let module = match kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program)) {
        Ok(m) => m,
        Err(err) => {
            compiler_error!("compile error: {}", err);
            eprintln!("{}", Red.paint(err));
            return ExitCode::FAILURE;
        }
    };

    match vm.run(&module) {
        Ok(result) => {
            format_result(vm, kernel.perm_interner().as_ref(), kernel.shape_forge().as_ref(), result);
            ExitCode::SUCCESS
        }
        Err(err) => {
            vm_error!("runtime error: {}", err);
            eprintln!("{}", Red.paint(err));
            ExitCode::FAILURE
        }
    }
}
fn not_implemented(command: &str) -> ExitCode {
    use ansi_term::Colour::Yellow;
    kernel_info!("command not yet implemented: {}", command);
    eprintln!("{}", Yellow.paint(format!("'{command}' is not yet implemented")));
    // 2 = 未实现，与 0=成功、1=运行失败区分，供脚本与 CI 识别。
    ExitCode::from(2)
}
