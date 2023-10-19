//! Different application scenarios to (hopefully) represent somewhat realistic scenarios.

use ethercrab::{
    slave_group::{Op, PreOp},
    Client, ClientConfig, PduStorage, SlaveGroup, Timeouts,
};
use futures_lite::StreamExt;
use std::{future::Future, time::Duration};

/// Maximum number of slaves that can be stored. This must be a power of 2 greater than 1.
const MAX_SLAVES: usize = 16;
/// Maximum PDU data payload size - set this to the max PDI size or higher.
const MAX_PDU_DATA: usize = 1100;
/// Maximum number of EtherCAT frames that can be in flight at any one time.
const MAX_FRAMES: usize = 64;

pub struct TestSettings {
    /// Ethernet NIC, e.g. `enp2s0`.
    nic: String,

    /// Whether we are running an RT kernel or not.
    is_rt: bool,

    /// If RT is enabled, this is the priority to set for thread that handles network IO.
    net_prio: u32,

    /// If RT is enabled, this is the priority to set for thread(s) that handle PDI tasks.
    task_prio: u32,

    /// Cycle time in microseconds.
    cycle_time_us: u32,
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

/// Single thread with TX/RX and one PDI loop running concurrently.
///
/// This function forces `smol` to not start an IO thread in the background, giving a more
/// representative worst case. In real code, one would either use a `static` `PduStorage`, or spawn
/// scoped threads so it's easier to use `smol::spawn`, `smol::block_on`, etc.
pub fn single_thread(settings: &TestSettings) -> Result<(), ethercrab::error::Error> {
    let storage = PduStorage::new();

    let (client, tx_rx) = create_client(&settings.nic, &storage);

    let local_ex = smol::LocalExecutor::new();

    local_ex.spawn(tx_rx).detach();

    futures_lite::future::block_on(local_ex.run(async move {
        let [group, ..] = create_groups(&client).await?;

        let mut group = group.into_op(&client).await.expect("PRE-OP -> OP");

        let mut tick = smol::Timer::interval(Duration::from_micros(settings.cycle_time_us.into()));

        // Collect 5000 samples
        for _ in 0..5000 {
            loop_tick(&mut group, &client).await;

            tick.next().await;
        }

        Ok(())
    }))
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
