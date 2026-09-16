//! 分块模式：按 chunk_size 切测试区间，逐块以子进程执行并解析 stdout 汇总行聚合。
//! 子进程经 current_exe 自派生，OXIDE_SKIP_UNTIL/OXIDE_MAX_TESTS/CHILD_CHUNK/ALLOW_FAIL_EXIT 环境变量窗口重建契约。

use std::process::Command;

/// 从子进程 stdout 中解析形如 `label   : N` 的汇总行。
fn parse_summary_count(stdout: &str, label: &str) -> Option<usize> {
    stdout.lines().find_map(|line| {
        let trimmed = line.trim_start();
        let rest = trimmed.strip_prefix(label)?;
        let value = rest.trim().split(' ').next()?;
        value.parse::<usize>().ok()
    })
}

/// 分块模式：按 chunk_size 把测试区间切块，逐块以子进程执行并汇总结果。
pub(crate) fn run_chunked(args: &[String], skip_until: usize, end_index: usize, chunk_size: usize) -> bool {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve current executable for chunked mode: {err}");
            return false;
        }
    };

    let mut aggregate_pass = 0usize;
    let mut aggregate_fail = 0usize;
    let mut aggregate_skip = 0usize;
    let mut chunk_start = skip_until;
    let mut chunk_id = 1usize;

    while chunk_start < end_index {
        let chunk_len = (end_index - chunk_start).min(chunk_size);
        eprintln!("chunk {chunk_id}: tests [{chunk_start}, {})", chunk_start + chunk_len);

        let output = match Command::new(&exe)
            .args(args.iter().skip(1))
            .env("OXIDE_SKIP_UNTIL", chunk_start.to_string())
            .env("OXIDE_MAX_TESTS", chunk_len.to_string())
            .env("OXIDE_TEST262_CHILD_CHUNK", "1")
            .env("OXIDE_TEST262_ALLOW_FAIL_EXIT", "1")
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                eprintln!("failed to run chunk {chunk_id}: {err}");
                return false;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        print!("{stdout}");
        eprint!("{stderr}");

        if !output.status.success() {
            eprintln!("chunk {chunk_id} crashed or aborted");
            return false;
        }

        aggregate_pass += parse_summary_count(&stdout, "pass   :").unwrap_or(0);
        aggregate_fail += parse_summary_count(&stdout, "fail   :").unwrap_or(0);
        aggregate_skip += parse_summary_count(&stdout, "skip   :").unwrap_or(0);

        chunk_start += chunk_len;
        chunk_id += 1;
    }

    let aggregate_total = aggregate_pass + aggregate_fail + aggregate_skip;
    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 chunked aggregate");
    println!("═══════════════════════════════════════");
    println!("  total  : {}", aggregate_total);
    println!("  pass   : {}", aggregate_pass);
    println!("  fail   : {}", aggregate_fail);
    println!("  skip   : {}", aggregate_skip);
    println!("═══════════════════════════════════════");

    aggregate_fail == 0
}
