//! Build provenance uses local tools only; no build dependencies or network access.
use std::{env, path::Path, process::Command};

fn main() {
    for path in [
        "src",
        "tests",
        "build.rs",
        "Cargo.toml",
        "Cargo.lock",
        "completions",
        "dev/completions.rs",
        "dev/completion_generator.rs",
        ".git/HEAD",
        ".git/index",
        ".git/refs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=VIEW_STORAGE_BUILD_REVISION");
    let revision = env::var("VIEW_STORAGE_BUILD_REVISION")
        .ok()
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .or_else(git_revision)
        .unwrap_or_else(|| "unknown".into());
    let compiler = env::var_os("RUSTC")
        .and_then(|rustc| Command::new(rustc).arg("--version").output().ok())
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    println!(
        "cargo:rustc-env=VIEW_STORAGE_REVISION={}",
        single_line(&revision)
    );
    println!(
        "cargo:rustc-env=VIEW_STORAGE_COMPILER={}",
        single_line(&compiler)
    );
}

fn git_revision() -> Option<String> {
    // A packaged crate must not adopt the revision of an unrelated parent checkout.
    if !Path::new(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut revision = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    let status = Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain",
            "--untracked-files=normal",
        ])
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }
    if !status.stdout.is_empty() {
        revision.push_str(" (modified)");
    }
    Some(revision)
}

fn single_line(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}
