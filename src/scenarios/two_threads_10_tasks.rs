use super::{
    create_client, create_groups, loop_tick, make_net_thread, make_task_thread, CycleMetadata,
    TestSettings,
};
use ethercrab::{self, PduStorage};
use futures_lite::StreamExt;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Two threads: 1 for tx/rx, the other for 10 concurrent tasks
pub fn two_threads_10_tasks(
    settings: &TestSettings,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    let storage = PduStorage::new();

    let (client, tx_rx) = create_client(&settings.nic, &storage);

    std::thread::scope(|s| {
        let client = Arc::new(client);

        let (net_tx, net_rx) = smol::channel::bounded(1);

        make_net_thread(settings)
            .spawn_scoped(s, move |_| {
                let local_ex = smol::LocalExecutor::new();

                futures_lite::future::block_on(local_ex.run(futures_lite::future::or(
                    tx_rx,
                    async {
                        net_rx.recv().await.unwrap();

                        Ok(())
                    },
                )))
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

        let res = make_task_thread(settings)
            .spawn_scoped_careless(s, move || {
                let local_ex = smol::LocalExecutor::new();

                let groups = futures_lite::future::block_on(
                    local_ex.run(futures::future::join_all(
                        groups
                            .into_iter()
                            .map(|group| task(group, &client, &settings)),
                    )),
                );

                let groups = groups.into_iter().flatten().collect::<Vec<_>>();

                Ok((groups, network_propagation_time_ns))
            })
            .unwrap()
            .join()
            .unwrap();

        // Stop net thread. Scoped thread hangs waiting on net task to join otherwise.
        net_tx.send_blocking(()).ok();

        res
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

    let iterations = 2000usize;

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
