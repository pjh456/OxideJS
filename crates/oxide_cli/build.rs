//! 构建脚本：取构建期 git 短哈希注入二进制（`GIT_COMMIT` 环境变量）。
//!
//! 哈希供 `--version` 自报，作为二进制新鲜度自证锚点（与 `git rev-parse --short HEAD`
//! 对照即可判定二进制是否落后于 HEAD）。无 git 环境（CI、解压源码）哈希缺省
//! `unknown`，构建不失败。

fn main() {
    // 构建脚本或源码变化时重跑，保持哈希与源码同步。
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // git 缺失或无提交时容缺，哈希缺省 "unknown"。
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_COMMIT={hash}");
}
