use crate::{
    scenarios::{run_all, TestSettings, DUMPS_PATH},
    system::{ethtool_usecs, hostname, is_rt_kernel, network_description, tunedadm_profile},
};
use clap::Parser;
use db::connect_and_init;
use scenarios::RunMetadata;
use sqlx::{query, types::Json};
use std::fs;
use tokio::runtime::Runtime;

mod db;
mod scenarios;
mod system;

/// Wireshark EtherCAT dump analyser
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Network interface name, e.g. "enp2s0".
    #[arg(long, short)]
    pub interface: String,

    /// Sets the priority for tests that use a separate thread for TX/RX.
    #[arg(long)]
    pub net_prio: u32,

    /// Sets the priority for tests that use a separate thread for tasks.
    ///
    /// All tasks will be given the same priority.
    #[arg(long)]
    pub task_prio: u32,

    /// Remove any previous dumps.
    #[arg(long)]
    pub clean: bool,

    /// Postgres DB URL, like `postgres://ethercrab:ethercrab@localhost:5432/dbname`.
    #[arg(
        long,
        default_value_t = String::from("postgres://ethercrab:ethercrab@localhost:5432/latency")
    )]
    pub db: String,

    /// Clean the database of all existing data before inserting new data.
    #[arg(long)]
    pub clean_db: bool,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let Args {
        interface,
        net_prio,
        task_prio,
        clean,
        db,
        clean_db,
    } = Args::parse();

    if clean {
        log::warn!("Removing all previous dumps");

        fs::remove_dir_all(DUMPS_PATH).expect("Failed to clean");
    }

    let is_rt = is_rt_kernel();
    let tuned_adm_profile = tunedadm_profile();
    let interface_description = network_description(&interface);
    let (tx_usecs, rx_usecs) = ethtool_usecs(&interface);
    let hostname = hostname();

    log::info!("Running tests");
    log::info!("- Hostname: {}", hostname);
    log::info!("- Interface: {} ({})", interface, interface_description);
    log::info!("- Realtime kernel: {}", if is_rt { "yes" } else { "no" });
    log::info!("- tuned-adm profile: {}", tuned_adm_profile);
    log::info!("- ethtool tx-usecs/rx-usecs: {}/{}", tx_usecs, rx_usecs);
    log::info!(
        "- Realtime priorities: net {}, task {}",
        net_prio,
        task_prio
    );

    if net_prio > 49 {
        log::warn!("Net priority {} is at or above kernel priority", net_prio);
    }

    if task_prio > 49 {
        log::warn!("Task priority {} is at or above kernel priority", task_prio);
    }

    if task_prio >= net_prio {
        log::warn!(
            "Task priority {} is at or above net priority {}. Ensure this is intentional",
            task_prio,
            net_prio
        );
    }

    // ---

    let settings = TestSettings {
        nic: interface,
        is_rt,
        net_prio,
        task_prio,
        hostname,
        // TODO: Another set of runs with 100us.
        cycle_time_us: 1000,
    };

    let results = run_all(&settings).expect("Runs failed");

    log::info!("All scenarios executed, ingesting results...");

    let rt = Runtime::new().unwrap();
    let handle = rt.handle();

    // Execute the future, blocking the current thread until completion
    handle
        .block_on(ingest(&settings, &db, clean_db, results))
        .expect("Ingest failed");
}

async fn ingest(
    settings: &TestSettings,
    db: &str,
    clean: bool,
    results: Vec<(&str, RunMetadata)>,
) -> anyhow::Result<()> {
    let db = connect_and_init(db).await?;

    if clean {
        // Postgres will cascade this through to the other tables
        query("truncate runs cascade").execute(&db).await?;
    }

    for (scenario_name, result) in results {
        // Insert a record into `runs`
        query(
            r#"insert into runs
            (date, scenario, name, hostname, propagation_time_ns, settings)
            values
            ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(result.date)
        .bind(scenario_name)
        .bind(&result.name)
        .bind(result.hostname)
        .bind(result.network_propagation_time_ns as i32)
        .bind(&Json(settings))
        .execute(&db)
        .await?;

        // Insert every cycle iteration stat
        for (counter, cycle) in result.cycle_metadata.iter().enumerate() {
            query(
                r#"insert into cycles
                (run, cycle, processing_time_ns, tick_wait_ns, cycle_time_delta_ns)
                values
                ($1, $2, $3, $4, $5)"#,
            )
            .bind(&result.name)
            .bind(counter as i32)
            .bind(cycle.processing_time_ns as i32)
            .bind(cycle.tick_wait_ns as i32)
            .bind(cycle.cycle_time_delta_ns as i32)
            .execute(&db)
            .await?;
        }

        // Process responses

        // Ingest responses
    }

    Ok(())
}
