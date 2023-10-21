//! Different application scenarios to (hopefully) represent somewhat realistic scenarios.

use chrono::{DateTime, Utc};
use ethercrab::{
    slave_group::{Op, PreOp},
    Client, ClientConfig, PduStorage, SlaveGroup, Timeouts,
};
use single_thread::single_thread;
use single_thread_10_tasks::single_thread_10_tasks;
use single_thread_2_tasks::single_thread_2_tasks;
use std::{
    fs,
    future::Future,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

mod single_thread;
mod single_thread_10_tasks;
mod single_thread_2_tasks;

/// Maximum number of slaves that can be stored. This must be a power of 2 greater than 1.
const MAX_SLAVES: usize = 16;
/// Maximum PDU data payload size - set this to the max PDI size or higher.
const MAX_PDU_DATA: usize = 1100;
/// Maximum number of EtherCAT frames that can be in flight at any one time.
const MAX_FRAMES: usize = 64;

pub const DUMPS_PATH: &str = "./dumps";

#[derive(serde::Serialize, Debug, Clone)]
pub struct TestSettings {
    /// Ethernet NIC, e.g. `enp2s0`.
    pub nic: String,

    /// Machine hostname.
    pub hostname: String,

    /// Whether we are running an RT kernel or not.
    pub is_rt: bool,

    /// If RT is enabled, this is the priority to set for thread that handles network IO.
    pub net_prio: u32,

    /// If RT is enabled, this is the priority to set for thread(s) that handle PDI tasks.
    pub task_prio: u32,

    /// Cycle time in microseconds.
    pub cycle_time_us: u32,
}

impl TestSettings {
    /// Get a hyphenated slug to insert into a filename, test name, etc.
    pub fn slug(&self) -> String {
        format!(
            "{}-{}-n{}-t{}-{}us",
            self.nic,
            if self.is_rt { "rt" } else { "nort" },
            self.net_prio,
            self.task_prio,
            self.cycle_time_us
        )
    }
}

/// Create an EtherCrab client and TX/RX task ready to be used and spawned respectively.
fn create_client<'sto>(
    ethercat_nic: &str,
    storage: &'sto PduStorage<MAX_FRAMES, MAX_PDU_DATA>,
) -> (
    Client<'sto>,
    impl Future<Output = Result<(), ethercrab::error::Error>> + 'sto,
) {
    let (tx, rx, pdu_loop) = storage.try_split().expect("Split");

    let client = Client::new(
        pdu_loop,
        Timeouts::default(),
        ClientConfig {
            dc_static_sync_iterations: 1000,
            ..ClientConfig::default()
        },
    );

    let tx_rx_task = ethercrab::std::tx_rx_task(ethercat_nic, tx, rx).expect("Spawn");

    (client, tx_rx_task)
}

type Group<S = PreOp> = SlaveGroup<1, 16, S>;
type Groups = [Group; 10];

/// Create a list of groups from discovered devices.
///
/// Each group may only have one device, with a PDI of up to 16 bytes.
async fn create_groups(client: &Client<'_>) -> Result<Groups, ethercrab::error::Error> {
    let mut index = 0;

    client
        .init::<MAX_SLAVES, _>(|groups: &Groups, _slave| {
            let g = &groups[index % groups.len()];

            index += 1;

            Ok(g)
        })
        .await
}

/// A single tick for a single group.
async fn loop_tick(group: &mut Group<Op>, client: &Client<'_>) {
    group.tx_rx(client).await.expect("TX/RX");

    // Increment every output byte for every slave device by one
    for slave in group.iter(client) {
        let (_i, o) = slave.io_raw();

        for byte in o.iter_mut() {
            *byte = byte.wrapping_add(1);
        }
    }
}

#[derive(Debug, Clone)]
pub struct CycleMetadata {
    /// Time spent processing TX/RX and process data.
    pub processing_time_ns: u32,

    /// Time spent waiting for the tick `await` call.
    pub tick_wait_ns: u32,

    /// The time from the same point in the previous cycle.
    ///
    /// Should be close or equal to configured cycle time.
    pub cycle_time_delta_ns: u32,

    /// Cycle number, starting from zero.
    pub cycle: usize,
}

#[derive(Debug, Clone)]
pub struct RunMetadata {
    pub date: DateTime<Utc>,

    /// Scenario name, e.g. `single-thread`.
    pub scenario: String,

    /// Run name.
    pub name: String,

    /// Metadata: computer hostname to use as an identifier.
    pub hostname: String,

    /// Data recorded for each process cycle in the scenario.
    ///
    /// Does not include anything before process cycle starts.
    pub cycle_metadata: Vec<CycleMetadata>,

    /// Time for a packet to reach the end of the network and come back, according to EtherCAT's DC
    /// system.
    pub network_propagation_time_ns: u32,

    /// Settings used for this run.
    pub settings: TestSettings,
}

fn run(
    settings: &TestSettings,
    scenario: impl Fn(&TestSettings) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error>,
    scenario_name: &str,
) -> Result<RunMetadata, ethercrab::error::Error> {
    let scenario_name = scenario_name.replace('_', "-");

    let now = Utc::now();

    let date_slug = now.timestamp();

    let name = format!(
        "{}-{}-{}-{}",
        scenario_name,
        settings.hostname,
        settings.slug(),
        date_slug
    );

    let dump_filename = dump_path(&name);

    let start = Instant::now();

    let mut tshark = {
        let mut cmd = std::process::Command::new("tshark");

        cmd.stdout(Stdio::null()).stderr(Stdio::null()).args(&[
            "-w",
            dump_filename.display().to_string().as_str(),
            "--interface",
            "enp2s0",
            "-f",
            "ether proto 0x88a4",
        ]);

        log::debug!("Running capture command {:?}", cmd);

        cmd.spawn().expect("Could not spawn tshark command")
    };

    // Let tshark settle in. It might miss packets if this delay is not here.
    std::thread::sleep(Duration::from_millis(200));

    log::info!(
        "Running scenario {}, saving to {}",
        scenario_name,
        dump_filename.display()
    );

    let (cycle_metadata, network_propagation_time_ns) = scenario(settings)?;

    std::thread::sleep(Duration::from_millis(100));

    // Stop tshark
    tshark.kill().expect("Failed to kill tshark");

    std::thread::sleep(Duration::from_millis(100));

    log::info!(
        "--> Collected {} process cycles in {} ms, network propagation time {} ns",
        cycle_metadata.len(),
        start.elapsed().as_millis(),
        network_propagation_time_ns
    );

    Ok(RunMetadata {
        date: now,
        hostname: settings.hostname.clone(),
        name,
        cycle_metadata,
        network_propagation_time_ns,
        scenario: scenario_name,
        settings: settings.clone(),
    })
}

/// Create a full canonicalised file path from a run name.
pub fn dump_path(name: &str) -> PathBuf {
    fs::create_dir_all(DUMPS_PATH).expect("Create dumps dir");

    let mut path = PathBuf::from(DUMPS_PATH)
        .canonicalize()
        .expect("Create dumps path");

    path.push(name);

    path.set_extension("pcapng");

    path
}

/// Run all scenarios sequentially while capturing network traffic in the background with `tshark`
/// for each one.
///
/// Network captures are saved to disk inside the `dumps/` folder.
pub fn run_all(
    settings: &TestSettings,
    filter: &Option<String>,
) -> Result<Vec<(&'static str, RunMetadata)>, ethercrab::error::Error> {
    let scenarios: Vec<(
        &dyn Fn(&TestSettings) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error>,
        &'static str,
    )> = vec![
        (&single_thread, "1thr"),
        (&single_thread_2_tasks, "1thr-2task"),
        (&single_thread_10_tasks, "1thr-10task"),
    ];

    scenarios
        .into_iter()
        .filter_map(|(scenario_fn, scenario_name)| {
            if let Some(filter) = filter {
                if scenario_name.contains(filter) {
                    Some(
                        run(settings, scenario_fn, &scenario_name)
                            .map(|result| (scenario_name, result)),
                    )
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Result<Vec<_>, _>>()
}

// TODO
//  thread_priority::ThreadBuilder::default()
//         .name("tx-rx-task")
//         // Might need to set `<user> hard rtprio 99` and `<user> soft rtprio 99` in `/etc/security/limits.conf`
//         // Check limits with `ulimit -Hr` or `ulimit -Sr`
//         .priority(ThreadPriority::Crossplatform(
//             ThreadPriorityValue::try_from(99u8).unwrap(),
//         ))
//         // NOTE: Requires a realtime kernel
//         .policy(ThreadSchedulePolicy::Realtime(
//             RealtimeThreadSchedulePolicy::Fifo,
//         ))
//         .spawn(move |_| {
//             let mut set = CpuSet::new();
//             set.set(0);

//             // Pin thread to 0th core
//             rustix::process::sched_setaffinity(None, &set).expect("set affinity");

//             let ex = LocalExecutor::new();

//             futures_lite::future::block_on(
//                 ex.run(tx_rx_task(&interface, tx, rx).expect("spawn TX/RX task")),
//             )
//             .expect("TX/RX task exited");
//         })
//         .unwrap();
