fn main() {
    // 编译期抓取仓库信息，供设置页“关于”区展示（打包后也能保留）。
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");
    let repo_url = run_git(&["remote", "get-url", "origin"]);
    println!("cargo:rustc-env=YULINK_REPO_URL={repo_url}");

    tauri_build::build()
}

/// 在仓库根目录执行 git 并取回第一行输出；失败返回空串（打包机无 git 时降级显示）。
fn run_git(args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg("..");
    cmd.args(args);
    match cmd.output() {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}
