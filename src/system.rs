use tokio::process::Command;

/// Determine whether the kernel has the RT patches enabled or not
pub async fn is_rt_kernel() -> bool {
    let cmd = Command::new("uname")
        .arg("-a")
        .output()
        .await
        .expect("uname command failed ");

    let out = String::from_utf8_lossy(&cmd.stdout);

    // Look for "-realtime" (Mint) or "-rt" (Debian)"
    out.contains("-realtime") || out.contains("-rt")
}

/// Read `tunedadm` profile
pub async fn tunedadm_profile() -> String {
    let cmd = Command::new("tuned-adm")
        .arg("active")
        .output()
        .await
        .expect("tuned-adm command failed ");

    let out = String::from_utf8_lossy(&cmd.stdout);

    out.split_whitespace()
        .last()
        .expect("No profile!")
        .to_string()
}

/// Get description of prescribed network device.
pub async fn network_description(search_device: &str) -> String {
    let cmd = Command::new("lshw")
        .arg("-class")
        .arg("network")
        .arg("-json")
        .output()
        .await
        .expect("lshw command failed ");

    let out: Vec<Device> = serde_json::from_slice(&cmd.stdout).expect("Invalid lshw JSON");

    let device = out
        .into_iter()
        .find(|device| device.logicalname == search_device)
        .expect("Could not find device");

    #[derive(Debug, serde::Deserialize)]
    struct Device {
        /// E.g. "RTL8125 2.5GbE Controller".
        product: String,
        /// Interface name, `enp2s0`, etc
        logicalname: String,
    }

    device.product
}

/// Get `tx-usecs` and `rx-usecs` `ethtool` statistics for the given interface
pub async fn ethtool_usecs(interface: &str) -> (u32, u32) {
    let cmd = Command::new("ethtool")
        .arg("-c")
        .arg(interface)
        .output()
        .await
        .expect("ethtool command failed ");

    let out = String::from_utf8_lossy(&cmd.stdout);

    let tx_usecs = out
        .lines()
        .find(|line| line.starts_with("tx-usecs"))
        .and_then(|line| line.split_whitespace().last()?.parse().ok())
        .expect("Did not find tx-usecs");

    let rx_usecs = out
        .lines()
        .find(|line| line.starts_with("rx-usecs"))
        .and_then(|line| line.split_whitespace().last()?.parse().ok())
        .expect("Did not find rx-usecs");

    (tx_usecs, rx_usecs)
}
