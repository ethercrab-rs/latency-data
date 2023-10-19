use tokio::process::Command;

/// Determine whether the kernel has the RT patches enabled or not
async fn is_rt_kernel() -> bool {
    let uname = Command::new("uname")
        .arg("-a")
        .output()
        .await
        .expect("uname command failed ");

    let out = String::from_utf8_lossy(&uname.stdout);

    // Look for "-realtime" (Mint) or "-rt" (Debian)"
    out.contains("-realtime") || out.contains("-rt")
}

/// Read `tunedadm` profile
async fn tunedadm_profile() -> String {
    let uname = Command::new("tuned-adm")
        .arg("active")
        .output()
        .await
        .expect("tuned-adm command failed ");

    let out = String::from_utf8_lossy(&uname.stdout);

    out.split_whitespace()
        .last()
        .expect("No profile!")
        .to_string()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let is_rt = is_rt_kernel().await;
    let tuned_adm_profile = tunedadm_profile().await;

    log::info!("Running tests");
    log::info!("- Realtime: {}", if is_rt { "yes" } else { "no" });
    log::info!("- tuned-adm profile: {}", tuned_adm_profile);
}
