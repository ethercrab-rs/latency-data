use super::{create_client, create_groups, loop_tick, CycleMetadata, TestSettings};
use crate::scenarios::{MAX_FRAMES, MAX_PDU_DATA};
use ethercrab::{self, Client, PduStorage};
use std::{
    mem::MaybeUninit,
    time::{Duration, Instant},
};

static mut STORAGE: MaybeUninit<PduStorage<MAX_FRAMES, MAX_PDU_DATA>> = MaybeUninit::uninit();

static mut CLIENT: MaybeUninit<Client<'static>> = MaybeUninit::uninit();

/// Just let tokio do whatever it wants with two tasks. We have `rt-multi-thread` turned on.
pub fn tokio_default(
    settings: &TestSettings,
) -> Result<(Vec<CycleMetadata>, u32), ethercrab::error::Error> {
    let settings = settings.clone();

    // SAFETY: Hilariously unsafe but I just want to do other things. As long as the previous run of
    // anything that uses `STORAGE` is done, this should/might be ok? I don't really care here tbh.
    // I just want to be able to call `split` more than once without it panicking :D
    let storage: &'static PduStorage<MAX_FRAMES, MAX_PDU_DATA> =
        unsafe { STORAGE.write(PduStorage::new()) };

    let rt = tokio::runtime::Runtime::new().expect("Runtime");

    rt.block_on(async {
        let (client, tx_rx) = create_client(&settings.nic, storage);

        // SAFETY: Get rekt
        let client = unsafe {
            CLIENT.write(client);

            CLIENT.assume_init_ref()
        };

        tokio::spawn(tx_rx);

        let mut groups = create_groups(&client).await?;

        // The time it takes to traverse to the end of the EtherCAT network and back again.
        let network_propagation_time_ns = groups
            .iter_mut()
            .flat_map(|group| group.iter(&client))
            .map(|device| device.propagation_delay())
            .max()
            .expect("Unable to compute prop time");

        let [group1, group2, ..] = groups;

        let f1 = tokio::spawn(task(group1, &client, settings.clone()));

        let f2 = tokio::spawn(task(group2, &client, settings));

        let (Ok(mut results1), Ok(mut results2)) = tokio::join!(f1, f2) else {
            unreachable!()
        };

        results1.append(&mut results2);

        Ok((results1, network_propagation_time_ns))
    })
}

async fn task(
    group: ethercrab::SlaveGroup<1, 16>,
    client: &ethercrab::Client<'static>,
    settings: TestSettings,
) -> Vec<CycleMetadata> {
    let mut group = group.into_op(client).await.expect("PRE-OP -> OP");
    let mut tick = tokio::time::interval(Duration::from_micros(settings.cycle_time_us.into()));
    let mut prev = Instant::now();

    let iterations = 5000usize;

    let mut cycles = Vec::with_capacity(iterations);

    for cycle in 0..iterations {
        let loop_start = Instant::now();

        loop_tick(&mut group, client).await;

        let processing_time_ns = loop_start.elapsed().as_nanos();

        tick.tick().await;

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
