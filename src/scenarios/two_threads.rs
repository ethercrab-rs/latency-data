use super::{
    create_client, create_groups, loop_tick, make_net_thread, make_task_thread, CycleMetadata,
    TestSettings,
};
use ethercrab::{self, PduStorage};
use futures_lite::{future, StreamExt};
use std::time::{Duration, Instant};

/// Two threads: one with the TX/RX task and another with a single group cycle.
pub fn two_threads(
    settings: &TestSettings,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    let storage = PduStorage::new();

    let (client, tx_rx) = create_client(&settings.nic, &storage);

    std::thread::scope(|s| {
        let (net_tx, net_rx) = smol::channel::bounded(1);

        make_net_thread(settings)
            .spawn_scoped(s, move |_| {
                let local_ex = smol::LocalExecutor::new();

                futures_lite::future::block_on(local_ex.run(future::or(tx_rx, async {
                    net_rx.recv().await.unwrap();

                    Ok(())
                })))
                // smol::block_on(tx_rx)
            })
            .expect("TX/RX thread");

        let res = make_task_thread(settings)
            .spawn_scoped(s, |_| {
                let local_ex = smol::LocalExecutor::new();

                futures_lite::future::block_on(local_ex.run(async move {
                    let mut groups = create_groups(&client).await?;

                    // The time it takes to traverse to the end of the EtherCAT network and back again.
                    let network_propagation_time_ns = groups
                        .iter_mut()
                        .flat_map(|group| group.iter(&client))
                        .map(|device| device.propagation_delay())
                        .max()
                        .expect("Unable to compute prop time");

                    let [group, ..] = groups;

                    let mut group = group.into_op(&client).await.expect("PRE-OP -> OP");

                    let mut tick =
                        smol::Timer::interval(Duration::from_micros(settings.cycle_time_us.into()));

                    let mut prev = Instant::now();

                    let iterations = 5000usize;
                    let mut cycles = Vec::with_capacity(iterations);

                    for cycle in 0..iterations {
                        let loop_start = Instant::now();

                        loop_tick(&mut group, &client).await;

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

                    Ok((cycles, network_propagation_time_ns))
                }))
            })
            .unwrap()
            .join()
            .unwrap();

        // Stop net thread. Scoped thread hangs waiting on net task to join otherwise.
        net_tx.send_blocking(()).ok();

        res
    })
}
