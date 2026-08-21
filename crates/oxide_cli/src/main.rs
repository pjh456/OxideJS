#![allow(clippy::arc_with_non_send_sync)]
#![allow(dead_code)]

use std::fs;
use std::process::ExitCode;
use std::sync::Arc;

use ansi_term::Colour::Red;
use clap::{Parser, Subcommand};
use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_compiler::compiler_error;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_kernel::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use oxide_kernel::string_forge::PermInterner;
use oxide_kernel::{kernel_error, kernel_info};
use oxide_log::{Level, SUBSYSTEM_COUNT};
use oxide_parser::Allocator;
use oxide_types::object::JsObject;
use oxide_vm::vm_error;
use oxide_vm::vm_pool::VmPool;
use oxide_vm::JsValue;
use oxide_vm::vm::Vm;

mod bench;

#[derive(Parser)]
#[command(name = "oxide", version, about = "OxideJS - Rust JavaScript engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, global = true)]
    verbose: bool,

    #[arg(short, long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    Eval {
        code: String,
    },
    Run {
        file: String,
        #[arg(long, default_value = "1")]
        repeat: u64,
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
        process: bool,
        #[arg(long)]
        update_baseline: bool,
        #[arg(long, default_value = "1000")]
        leak_check_interval: usize,
    },
    Test {
        suite: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Eval { code }) => {
            let kernel = make_kernel(cli.verbose, cli.quiet);
            let pool = make_pool(&kernel);
            eval(&code, &kernel, &pool)
        }
        Some(Commands::Run { file, repeat }) => {
            let kernel = make_kernel(cli.verbose, cli.quiet);
            let pool = make_pool(&kernel);
            for n in 0..repeat {
                let code = run(&file, &kernel, &pool);
                if code != ExitCode::SUCCESS && code != ExitCode::FAILURE {
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
            process,
            update_baseline,
            leak_check_interval,
        }) => {
            let kernel = make_kernel(false, false);
            let pool = make_pool(&kernel);
            let config = bench::BenchConfig {
                mode: mode.unwrap_or_else(|| "js".to_string()),
                filter,
                warmup,
                iterations,
                process,
                update_baseline,
                leak_check_interval,
            };
            bench::run_benchmarks(config, kernel, pool)
        }
        Some(Commands::Test { .. }) => not_implemented("test"),
        None => repl(),
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

fn eval(code: &str, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>) -> ExitCode {
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
    match guard.vm_mut().run(&module) {
        Ok(result) => {
            format_result(guard.vm(), kernel.perm_interner().as_ref(), kernel.shape_forge().as_ref(), result);
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

fn format_js_value(
    vm: &oxide_vm::vm::Vm, string_forge: &PermInterner, shape_forge: &ShapeForge, val: JsValue,
) -> String {
    if val.is_string() {
        // SAFETY: val 已确认是字符串值。
        let s = unsafe { (*val.as_string_ptr()).to_owned_string() };
        format!("\"{s}\"")
    } else if val.is_bigint() {
        format!("{}", vm.bigint_value(val))
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        if obj.is_promise_obj() {
            // 已 settle 的 Promise 打印其结算值（便于 eval 观察微任务结果）。
            return match oxide_vm::promise::promise_settled_value(obj) {
                Some((true, v)) => format_js_value(vm, string_forge, shape_forge, v),
                Some((false, v)) => {
                    format!("Promise {{ <rejected> {} }}", format_js_value(vm, string_forge, shape_forge, v))
                }
                None => "Promise { <pending> }".to_string(),
            };
        }
        if obj.is_function() {
            "[Function]".to_string()
        } else if obj.is_array() {
            format_array(vm, string_forge, shape_forge, obj)
        } else {
            format_object(vm, string_forge, shape_forge, obj)
        }
    } else if val.is_undefined() {
        "undefined".to_string()
    } else {
        format!("{val}")
    }
}

fn format_object(
    vm: &oxide_vm::vm::Vm, string_forge: &PermInterner, shape_forge: &ShapeForge, obj: &JsObject,
) -> String {
    let mut entries = Vec::new();
    let shape_id = obj.shape_id();
    let mut shape_ids = Vec::new();
    let mut cursor = Some(shape_id);
    while let Some(id) = cursor {
        if id == EMPTY_SHAPE_ID {
            break;
        }
        if let Some(shape) = shape_forge.get_shape(id) {
            cursor = shape.parent;
            if shape.property_name != u32::MAX {
                shape_ids.push(id);
            }
        } else {
            break;
        }
    }
    let mut pos: u32 = 0;
    for id in shape_ids.iter().rev() {
        if let Some(shape) = shape_forge.get_shape(*id) {
            if shape.property_name != 0 {
                let prop_val = obj.get_prop_at(pos);
                if prop_val.is_undefined() {
                    pos += 1;
                    continue;
                }
                let name = string_forge.lookup(shape.property_name).unwrap_or_default();
                let val_str = format_js_value(vm, string_forge, shape_forge, prop_val);
                entries.push(format!("\"{name}\": {val_str}"));
            }
        }
        pos += 1;
    }
    format!("{{{}}}", entries.join(", "))
}

fn format_array(
    vm: &oxide_vm::vm::Vm, string_forge: &PermInterner, shape_forge: &ShapeForge, obj: &JsObject,
) -> String {
    let len = obj.prop_vec_len();
    let mut items = Vec::new();
    for i in 0..len {
        let val = obj.get_prop_at(i);
        items.push(format_js_value(vm, string_forge, shape_forge, val));
    }
    format!("[{}]", items.join(", "))
}

fn run(file: &str, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>) -> ExitCode {
    match fs::read_to_string(file) {
        Ok(source) => eval(&source, kernel, pool),
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
                    // eval_repl already printed error; keep buffer cleared.
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
    ExitCode::SUCCESS
}
