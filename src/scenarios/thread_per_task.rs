use super::{
    create_client, create_groups, loop_tick, make_net_thread, make_task_thread, CycleMetadata,
    TestSettings,
};
use ethercrab::{self, PduStorage};
use futures_lite::{future, StreamExt};
use std::{
    sync::Arc,
    thread::ScopedJoinHandle,
    time::{Duration, Instant},
};

// Start 1 tx/rx thread and 1 task thread.
pub fn two_threads(
    settings: &TestSettings,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    inner(settings, 1)
}

// Start 1 tx/rx thread and 2 task threads.
pub fn three_threads(
    settings: &TestSettings,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    inner(settings, 2)
}

// Start 1 tx/rx thread and 10 task threads.
pub fn eleven_threads(
    settings: &TestSettings,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    inner(settings, 10)
}

fn inner(
    settings: &TestSettings,
    num_tasks: usize,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    let storage = PduStorage::new();

    let (client, tx_rx) = create_client(&settings.nic, &storage);

    std::thread::scope(|s| {
        let client = Arc::new(client);

        let (net_tx, net_rx) = smol::channel::bounded(1);

        make_net_thread(settings)
            .spawn_scoped(s, move |_| {
                let local_ex = smol::LocalExecutor::new();

                futures_lite::future::block_on(local_ex.run(future::or(tx_rx, async {
                    net_rx.recv().await.ok();

                    Ok(())
                })))
                // smol::block_on(tx_rx)
            })
            .expect("TX/RX thread");

        let mut groups = smol::block_on(create_groups(&client))?;

        // The time it takes to traverse to the end of the EtherCAT network and back again.
        let network_propagation_time_ns = groups
            .iter_mut()
            .flat_map(|group| group.iter(&client))
            .map(|device| device.propagation_delay())
            .max()
            .expect("Unable to compute prop time");

        let groups = groups.into_iter().take(num_tasks).collect::<Vec<_>>();

        let handles = groups
            .into_iter()
            .map(|group| {
                let client = client.clone();

                make_task_thread(settings)
                    .spawn_scoped_careless(s, move || {
                        let local_ex = smol::LocalExecutor::new();

                        futures_lite::future::block_on(
                            local_ex.run(task(group, &client, &settings)),
                        )
                    })
                    .unwrap()
            })
            .collect::<Vec<ScopedJoinHandle<'_, Vec<CycleMetadata>>>>();

        let results = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap().into_iter())
            .collect::<Vec<CycleMetadata>>();

        // Stop net thread. Scoped thread hangs waiting on net task to join otherwise.
        net_tx.send_blocking(()).ok();

        Ok((results, network_propagation_time_ns))
    })
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

    for cycle in 0..iterations {
        let loop_start = Instant::now();

        loop_tick(&mut group, client).await;

        let processing_time_ns = loop_start.elapsed().as_nanos();

        tick.next().await;

        let tick_wait_ns = loop_start.elapsed().as_nanos() - processing_time_ns;
        let cycle_time_delta_ns = prev.elapsed().as_nanos();

        cycles.push(CycleMetadata {
            cycle,
            processing_time_ns: processing_time_ns as u32,
            tick_wait_ns: tick_wait_ns as u32,
            cycle_time_delta_ns: cycle_time_delta_ns as u32,
        });

        prev = Instant::now();
    }

    cycles
}
