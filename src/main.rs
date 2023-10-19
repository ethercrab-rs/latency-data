use crate::system::{ethtool_usecs, is_rt_kernel, network_description, tunedadm_profile};
use clap::Parser;

mod system;

/// Wireshark EtherCAT dump analyser
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Network interface name, e.g. "enp2s0".
    #[arg(long, short)]
    pub interface: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    let is_rt = is_rt_kernel().await;
    let tuned_adm_profile = tunedadm_profile().await;
    let interface_description = network_description(&args.interface).await;
    let (tx_usecs, rx_usecs) = ethtool_usecs(&args.interface).await;

    log::info!("Running tests");
    log::info!(
        "- Interface: {} ({})",
        args.interface,
        interface_description
    );
    log::info!("- Realtime kernel: {}", if is_rt { "yes" } else { "no" });
    log::info!("- tuned-adm profile: {}", tuned_adm_profile);
    log::info!("- ethtool tx-usecs/rx-usecs: {}/{}", tx_usecs, rx_usecs);
}
