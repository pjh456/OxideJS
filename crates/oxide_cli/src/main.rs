use std::fs;
use std::process::ExitCode;

use ansi_term::Colour::Red;
use clap::{Parser, Subcommand};
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

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
        Some(Commands::Eval { code }) => eval(&code),
        Some(Commands::Run { file }) => run(&file),
        Some(Commands::Compile { expr, file }) => compile(expr, file),
        Some(Commands::Bench { .. }) => not_implemented("bench"),
        Some(Commands::Test { .. }) => not_implemented("test"),
        None => repl(),
    }
}

fn eval(code: &str) -> ExitCode {
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
    let module = match compiler.compile(&program) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("{}", Red.paint(err));
            return ExitCode::FAILURE;
        }
    };

    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}", Red.paint(err));
            ExitCode::FAILURE
        }
    }
}

fn run(file: &str) -> ExitCode {
    match fs::read_to_string(file) {
        Ok(source) => eval(&source),
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

    loop {
        match rl.readline("oxide> ") {
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
                eval(trimmed);
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
