#![allow(clippy::arc_with_non_send_sync)]

use std::fs;
use std::process::ExitCode;
use std::sync::Arc;

use ansi_term::Colour::Red;
use clap::{Parser, Subcommand};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_kernel::string_forge::StringForge;
use oxide_parser::Allocator;
use oxide_vm::vm_pool::VmPool;
use oxide_vm::JsValue;

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
    },
    Compile {
        #[arg(short = 'e')]
        expr: Option<String>,
        file: Option<String>,
    },
    Bench {
        suite: Option<String>,
    },
    Test {
        suite: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Eval { code }) => {
            let kernel = make_kernel();
            let pool = make_pool(&kernel);
            eval(&code, &kernel, &pool)
        }
        Some(Commands::Run { file }) => {
            let kernel = make_kernel();
            let pool = make_pool(&kernel);
            run(&file, &kernel, &pool)
        }
        Some(Commands::Compile { expr, file }) => compile(expr, file),
        Some(Commands::Bench { .. }) => not_implemented("bench"),
        Some(Commands::Test { .. }) => not_implemented("test"),
        None => repl(),
    }
}

fn make_kernel() -> Arc<OxideKernel> {
    let kernel = Arc::new(OxideKernel::new(KernelConfig::standard()));
    oxide_vm::vm::init_kernel_builtins(&kernel);
    kernel
}

fn make_pool(kernel: &Arc<OxideKernel>) -> Arc<VmPool> {
    VmPool::new(
        Arc::clone(kernel),
        kernel.config().min_pool_size,
        kernel.config().max_pool_size,
    )
}

fn eval(code: &str, kernel: &Arc<OxideKernel>, pool: &Arc<VmPool>) -> ExitCode {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, code) {
        Ok(p) => p,
        Err(errors) => {
            for err in &errors {
                eprintln!("{}", Red.paint(err.to_string()));
            }
            return ExitCode::FAILURE;
        }
    };

    let compiler = Compiler::new();
    let module = match kernel.code_forge().get_or_compile(&program, &compiler) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("{}", Red.paint(err));
            return ExitCode::FAILURE;
        }
    };

    let mut guard = pool.spawn();
    match guard.vm_mut().run(&module) {
        Ok(result) => {
            format_result(kernel.string_forge().as_ref(), result);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}", Red.paint(err));
            ExitCode::FAILURE
        }
    }
}

fn format_result(string_forge: &StringForge, val: JsValue) {
    if val.is_string() {
        if let Some(s) = string_forge.lookup(val.as_string_index()) {
            println!("\"{s}\"");
        } else {
            println!("{val}");
        }
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        if obj.is_function() {
            println!("[Function]");
        } else {
            print!("{{");
            let count = obj.prop_count() as usize;
            let mut first = true;
            for i in 0..count.min(4) {
                let prop_val = obj.get_prop_at(i as u8);
                if prop_val.is_undefined() {
                    continue;
                }
                if !first {
                    print!(", ");
                }
                first = false;
                print!("{:?}: ", i);
                print_value(string_forge, prop_val);
            }
            println!("}}");
        }
    } else if val.is_undefined() {
        println!("undefined");
    } else {
        println!("{val}");
    }
}

fn print_value(string_forge: &StringForge, val: JsValue) {
    if val.is_string() {
        if let Some(s) = string_forge.lookup(val.as_string_index()) {
            print!("\"{s}\"");
        } else {
            print!("{val}");
        }
    } else {
        print!("{val}");
    }
}

fn run(file: &str, kernel: &Arc<OxideKernel>, pool: &Arc<VmPool>) -> ExitCode {
    match fs::read_to_string(file) {
        Ok(source) => eval(&source, kernel, pool),
        Err(err) => {
            eprintln!("{}", Red.paint(format!("Cannot read {file}: {err}")));
            ExitCode::FAILURE
        }
    }
}

fn compile(expr: Option<String>, file: Option<String>) -> ExitCode {
    let source = if let Some(code) = expr {
        code
    } else if let Some(path) = file {
        match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("{}", Red.paint(format!("Cannot read {path}: {err}")));
                return ExitCode::FAILURE;
            }
        }
    } else {
        eprintln!(
            "{}",
            Red.paint("compile requires -e '<code>' or a file path")
        );
        return ExitCode::FAILURE;
    };

    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, &source) {
        Ok(p) => p,
        Err(errors) => {
            for err in &errors {
                eprintln!("{}", Red.paint(err.to_string()));
            }
            return ExitCode::FAILURE;
        }
    };

    let compiler = Compiler::new();
    match compiler.compile(&program) {
        Ok(module) => {
            print!("{module}");
            ExitCode::SUCCESS
        }
        Err(err) => {
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
            eprintln!("{}", Red.paint(format!("Failed to start REPL: {err}")));
            return ExitCode::FAILURE;
        }
    };

    let kernel = make_kernel();
    let pool = make_pool(&kernel);
    let mut source = String::new();
    let mut input_buf = String::new();

    loop {
        let prompt = if input_buf.is_empty() {
            "oxide> "
        } else {
            "...> "
        };
        match rl.readline(prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == ".exit" || trimmed == ".quit" {
                    println!("exit");
                    return ExitCode::SUCCESS;
                }
                rl.add_history_entry(trimmed).ok();

                if !input_buf.is_empty() {
                    input_buf.push('\n');
                }
                input_buf.push_str(trimmed);

                let balance = bracket_balance(&input_buf);
                if balance > 0 {
                    continue;
                }

                let mut full_code = source.clone();
                if !full_code.is_empty() {
                    full_code.push(';');
                }
                full_code.push_str(&input_buf);

                let result = eval(&full_code, &kernel, &pool);
                input_buf.clear();

                if result == ExitCode::SUCCESS {
                    source = full_code;
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
                eprintln!("{}", Red.paint(format!("REPL error: {err}")));
                return ExitCode::FAILURE;
            }
        }
    }
}
fn not_implemented(command: &str) -> ExitCode {
    use ansi_term::Colour::Yellow;
    eprintln!(
        "{}",
        Yellow.paint(format!("'{command}' is not yet implemented"))
    );
    ExitCode::SUCCESS
}
