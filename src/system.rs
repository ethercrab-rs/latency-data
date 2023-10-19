use tokio::process::Command;

/// Determine whether the kernel has the RT patches enabled or not
pub async fn is_rt_kernel() -> bool {
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
pub async fn tunedadm_profile() -> String {
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
