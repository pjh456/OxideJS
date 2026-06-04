use std::fs;
use std::process::ExitCode;

use ansi_term::Colour::Red;
use clap::{Parser, Subcommand};
use oxide_parser::Allocator;

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
    Eval { code: String },
    Run { file: String },
    Bench { suite: Option<String> },
    Test { suite: Option<String> },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Eval { code }) => eval(&code),
        Some(Commands::Run { file }) => run(&file),
        Some(Commands::Bench { .. }) => not_implemented("bench"),
        Some(Commands::Test { .. }) => not_implemented("test"),
        None => repl(),
    }
}

fn eval(code: &str) -> ExitCode {
    let allocator = Allocator::default();
    match oxide_parser::parse(&allocator, code) {
        Ok(program) => {
            println!("{program:#?}");
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for err in &errors {
                eprintln!("{}", Red.paint(err.to_string()));
            }
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
