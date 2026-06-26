pub fn run_rust_bench(filter: Option<&str>) -> std::process::ExitCode {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["bench", "-p", "oxide_vm"]);
    if let Some(name) = filter {
        cmd.args(["--bench", name]);
    }
    match cmd.status() {
        Ok(status) => {
            if status.success() {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("Failed to run cargo bench: {}", e);
            std::process::ExitCode::from(1)
        }
    }
}
