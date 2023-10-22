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

    // /// Sets the priority for tests that use a separate thread for TX/RX.
    // #[arg(long)]
    // pub net_prio: u32,

    // /// Sets the priority for tests that use a separate thread for tasks.
    // ///
    // /// All tasks will be given the same priority.
    // #[arg(long)]
    // pub task_prio: u32,
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

    /// Filter scenarios to those containing this string.
    #[arg(long)]
    pub filter: Option<String>,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,sqlx=error"))
        .init();

    let Args {
        interface,
        // net_prio,
        // task_prio,
        clean,
        db,
        clean_db,
        repeat,
        filter,
    } = Args::parse();

    if clean {
        log::warn!("Removing all previous dumps");

        // Ignore errors
        let _ = fs::remove_dir_all(DUMPS_PATH);
    }

    if let Some(filter) = filter.as_ref() {
        log::info!("Filtering scenarios with filter {:?}", filter);
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
    // log::info!(
    //     "- Realtime priorities: net {}, task {}",
    //     net_prio,
    //     task_prio
    // );

    // if net_prio > 49 {
    //     log::warn!("Net priority {} is at or above kernel priority", net_prio);
    // }

    // if task_prio > 49 {
    //     log::warn!("Task priority {} is at or above kernel priority", task_prio);
    // }

    // if task_prio >= net_prio {
    //     log::warn!(
    //         "Task priority {} is at or above net priority {}. Ensure this is intentional",
    //         task_prio,
    //         net_prio
    //     );
    // }

    // ---

    let mut results = Vec::new();

    // Priority combinations for SCHED_FIFO
    let prios = if is_rt {
        vec![
            // Use defaults
            (0, 0),
            // Just below kernel prio (50)
            (48, 49),
            (49, 48),
            // Get really mean
            (90, 91),
            (91, 90),
        ]
    } else {
        // These won't be set if RT is disabled so we'll just default to 0,0 to run the suite once.
        vec![(0, 0)]
    };

    for (task_prio, net_prio) in prios {
        if is_rt {
            log::info!(
                "Running with RT priorities task {}, net {}",
                task_prio,
                net_prio
            );
        }

        let settings = TestSettings {
            nic: interface.clone(),
            is_rt,
            net_prio,
            task_prio,
            hostname: hostname.clone(),
            cycle_time_us: 1000,
        };

        for _ in 0..repeat {
            results.extend(run_all(&settings, &filter).expect("1000us runs failed"));
        }

        let settings = TestSettings {
            cycle_time_us: 100,
            ..settings
        };

        for _ in 0..repeat {
            results.extend(run_all(&settings, &filter).expect("100us runs failed"));
        }
    }

    log::info!("All scenarios executed, ingesting results...");

    let rt = Runtime::new().unwrap();
    let handle = rt.handle();

    // Execute the future, blocking the current thread until completion
    handle
        .block_on(ingest(&db, clean_db, results))
        .expect("Ingest failed");
}

async fn ingest(db: &str, clean: bool, results: Vec<(&str, RunMetadata)>) -> anyhow::Result<()> {
    let db = connect_and_init(db).await?;

    if clean {
        // Postgres will cascade this through to the other tables
        query("truncate runs cascade").execute(&db).await?;
    }

    for (scenario_name, result) in results {
        log::info!(
            "Ingesting data for scenario {}, run {}",
            scenario_name,
            result.name
        );

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
        .bind(&Json(result.settings))
        .execute(&db)
        .await?;

        // Insert every cycle iteration stat
        for chunk in result.cycle_metadata.chunks(5000) {
            QueryBuilder::new(
                r#"insert into cycles
                (run, cycle, processing_time_ns, tick_wait_ns, cycle_time_delta_ns) "#,
            )
            .push_values(chunk.iter(), |mut b, cycle| {
                b.push_bind(&result.name)
                    .push_bind(cycle.cycle as i32)
                    .push_bind(cycle.processing_time_ns as i32)
                    .push_bind(cycle.tick_wait_ns as i32)
                    .push_bind(cycle.cycle_time_delta_ns as i32);
            })
            .build()
            .execute(&db)
            .await?;
        }

        log::info!("--> Cycles done");

        // Skip all init packets by looking for a first LRW, which is a good canary for cyclic data
        // start. Once found, only look for LRW frames.
        let reader = PcapFile::new(&dump_path(&result.name))
            .skip_while(|packet| !matches!(packet.command, Command::Write(Writes::Lrw { .. })))
            .filter(|packet| matches!(packet.command, Command::Write(Writes::Lrw { .. })));

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
                let len = scratch.len();

                // Find last sent PDU with this receive PDU's same index
                let sent = scratch
                    .iter_mut()
                    .rev()
                    .find(|stat| stat.index == packet.index as i16)
                    .expect(&format!(
                        "Could not find sent packet {} in {} prev packets",
                        packet.index, len
                    ));

                sent.rx_time_ns = (packet.time - start_offset).as_nanos() as i64;
                sent.delta_time_ns = (sent.rx_time_ns - sent.tx_time_ns) as i32;
            }
        }

        let mut acq = db.acquire().await.unwrap();

        let mut copy = acq.copy_in_raw("copy frames (run, packet_number, index, command, tx_time_ns, rx_time_ns, delta_time_ns) from stdin (format csv, delimiter '|')").await.expect("COPY cmd");

        let rows = scratch.into_iter().map(
            |Packet {
                 packet_number,
                 index,
                 command,
                 tx_time_ns,
                 rx_time_ns,
                 delta_time_ns,
             }| {
                format!(
                    "{}|{}|{}|{}|{}|{}|{}\n",
                    result.name,
                    packet_number,
                    index,
                    command,
                    tx_time_ns,
                    rx_time_ns,
                    delta_time_ns,
                )
            },
        );

        for row in rows {
            copy.read_from(row.as_bytes()).await.expect("COPY row");
        }

        copy.finish().await.unwrap();

        log::info!("--> Frames done");
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
