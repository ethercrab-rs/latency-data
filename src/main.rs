use crate::{
    scenarios::{run_all, TestSettings, DUMPS_PATH},
    system::{ethtool_usecs, hostname, is_rt_kernel, network_description, tunedadm_profile},
};
use clap::Parser;
use db::connect_and_init;
use dump_analyser::PcapFile;
use ethercrab::{Command, Writes};
use scenarios::{dump_path, RunMetadata};
use sqlx::{query, types::Json, QueryBuilder};
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

    /// Number of times to run each scenario.
    #[arg(long, default_value_t = 1)]
    pub repeat: u32,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,sqlx=error"))
        .init();

    let Args {
        interface,
        net_prio,
        task_prio,
        clean,
        db,
        clean_db,
        repeat,
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

    let mut results = Vec::new();

    for _ in 0..repeat {
        results.extend(run_all(&settings).expect("Runs failed"));
    }

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

        let mut counter = 0;

        // Insert every cycle iteration stat
        for chunk in result.cycle_metadata.chunks(5000) {
            QueryBuilder::new(
                r#"insert into cycles
                (run, cycle, processing_time_ns, tick_wait_ns, cycle_time_delta_ns) "#,
            )
            .push_values(chunk.iter(), |mut b, cycle| {
                b.push_bind(&result.name)
                    .push_bind(counter as i32)
                    .push_bind(cycle.processing_time_ns as i32)
                    .push_bind(cycle.tick_wait_ns as i32)
                    .push_bind(cycle.cycle_time_delta_ns as i32);

                counter += 1;
            })
            .build()
            .execute(&db)
            .await?;
        }

        // Skip all init packets by looking for a first LRW, which is a good canary for cyclic data
        // start.
        let reader = PcapFile::new(&dump_path(&result.name))
            .skip_while(|packet| !matches!(packet.command, Command::Write(Writes::Lrw { .. })));

        let cycle_packets = reader.collect::<Vec<_>>();
        let first_packet = cycle_packets.first().expect("Empty dump");

        // Make all TX/RX times relative to first unfiltered packet
        let start_offset = first_packet.time;

        // A vec to collect sent/received PDU pairs into a single item with metadata
        let mut scratch = Vec::new();

        for packet in cycle_packets {
            // Newly sent PDU
            if packet.from_master {
                scratch.push(Packet {
                    packet_number: packet.wireshark_packet_number as i32,
                    index: packet.index as i16,
                    tx_time_ns: (packet.time - start_offset).as_nanos() as i64,
                    rx_time_ns: 0,
                    delta_time_ns: 0,
                    command: packet.command.to_string(),
                });
            }
            // Response to existing sent PDU
            else {
                // Find last sent PDU with this receive PDU's same index
                let sent = scratch
                    .iter_mut()
                    .rev()
                    .find(|stat| stat.index == packet.index as i16)
                    .expect(&format!("Could not find sent packet {}", packet.index));

                sent.rx_time_ns = (packet.time - start_offset).as_nanos() as i64;
                sent.delta_time_ns = (sent.rx_time_ns - sent.tx_time_ns) as i32;
            }
        }

        for chunk in scratch.chunks(5000) {
            QueryBuilder::new(
                r#"insert into frames
            (run, packet_number, index, command, tx_time_ns, rx_time_ns, delta_time_ns) "#,
            )
            .push_values(chunk, |mut b, packet| {
                b.push_bind(&result.name)
                    .push_bind(packet.packet_number)
                    .push_bind(packet.index)
                    .push_bind(&packet.command)
                    .push_bind(packet.tx_time_ns)
                    .push_bind(packet.rx_time_ns)
                    .push_bind(packet.delta_time_ns);
            })
            .build()
            .execute(&db)
            .await?;
        }
    }

    Ok(())
}

/// Database representation of a TX/RX cycle.
#[derive(Debug)]
struct Packet {
    packet_number: i32,
    index: i16,
    command: String,
    tx_time_ns: i64,
    rx_time_ns: i64,
    delta_time_ns: i32,
}
