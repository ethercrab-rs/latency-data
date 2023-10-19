use tokio::process::Command;

/// Determine whether the kernel has the RT patches enabled or not
async fn is_rt_kernel() -> bool {
    // Look for ""-realtime" (Mint) or "-rt" (Debian)"

    let uname = Command::new("uname")
        .arg("-a")
        .output()
        .await
        .expect("uname command failed ");

    let out = String::from_utf8_lossy(&uname.stdout);

    out.contains("-realtime") || out.contains("-rt")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let is_rt = is_rt_kernel().await;

    log::info!("Running tests");
    log::info!("- Realtime: {}", if is_rt { "yes" } else { "no" });
}
