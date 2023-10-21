use super::{create_client, create_groups, loop_tick, CycleMetadata, TestSettings};
use ethercrab::{self, PduStorage};
use futures_lite::StreamExt;
use std::time::{Duration, Instant};

/// Single thread with TX/RX and 2 PDI loops running concurrently.
///
/// This function forces `smol` to not start an IO thread in the background, giving a more
/// representative worst case. In real code, one would either use a `static` `PduStorage`, or spawn
/// scoped threads so it's easier to use `smol::spawn`, `smol::block_on`, etc.
pub fn single_thread_2_tasks(
    settings: &TestSettings,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    let storage = PduStorage::new();

    let (client, tx_rx) = create_client(&settings.nic, &storage);

    let local_ex = smol::LocalExecutor::new();

    local_ex.spawn(tx_rx).detach();

    let mut groups = futures_lite::future::block_on(local_ex.run(create_groups(&client)))?;

    // The time it takes to traverse to the end of the EtherCAT network and back again.
    let network_propagation_time_ns = groups
        .iter_mut()
        .flat_map(|group| group.iter(&client))
        .map(|device| device.propagation_delay())
        .max()
        .expect("Unable to compute prop time");

    let [group1, group2, ..] = groups;

    let f1 = local_ex.spawn(task(group1, &client, settings));

    let f2 = local_ex.spawn(task(group2, &client, settings));

    let (mut results1, mut results2) =
        futures_lite::future::block_on(local_ex.run(futures_lite::future::zip(f1, f2)));

    results1.append(&mut results2);

    Ok((results1, network_propagation_time_ns))
}

async fn task(
    group: ethercrab::SlaveGroup<1, 16>,
    client: &ethercrab::Client<'_>,
    settings: &TestSettings,
) -> Vec<CycleMetadata> {
    let mut group = group.into_op(client).await.expect("PRE-OP -> OP");
    let mut tick = smol::Timer::interval(Duration::from_micros(settings.cycle_time_us.into()));
    let mut prev = Instant::now();

    let iterations = 5000usize;

    let mut cycles = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let loop_start = Instant::now();

        loop_tick(&mut group, client).await;

        let processing_time_ns = loop_start.elapsed().as_nanos();

        tick.next().await;

        let tick_wait_ns = loop_start.elapsed().as_nanos() - processing_time_ns;
        let cycle_time_delta_ns = prev.elapsed().as_nanos();

        cycles.push(CycleMetadata {
            processing_time_ns: processing_time_ns as u32,
            tick_wait_ns: tick_wait_ns as u32,
            cycle_time_delta_ns: cycle_time_delta_ns as u32,
        });

        prev = Instant::now();
    }

    cycles
}
