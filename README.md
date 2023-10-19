# Latency data gathering

Tools and documentation of various tests to run against a system to gather latency/jitter results
from various system tuning parameters.

Designed to be analysed with [dump-analyser](https://github.com/ethercrab-rs/dump-analyser), so data
is imported into Postgres as that's what I'm familiar with.

# The setup

Debian 12.2.0 netinst, with no desktop environments installed.

BTRFS (because snapshots, but I can't make these work so this may as well be ext4 ugh)

`rustc` 1.73.0

EtherCrab opens a raw socket like:

```rust
libc::socket(
    // Ethernet II frames
    libc::AF_PACKET,
    libc::SOCK_RAW | libc::SOCK_NONBLOCK,
    0x88a4u16.to_be() as i32,
);
```

Cross compiled from Windows 11 for `x86_64-unknown-linux-gnu`. Binary will be run as root.

Only other deliberate task will be `tshark`, started by the test binary.

Each test will be run 3 times.

## Hardware

- Integrated NIC
  - SSH will be run through this most of the time, unless it's in use by a test, then i350 port 3
    will be used.
- Intel i350 (Dell, port 0)
- Intel i210
- Intel i225-v

## Scenarios

- Normal kernel
- Normal kernel with ethtool
- Normal kernel with tunedadm
- Normal kernel with tunedadm + ethtool

- RT kernel no prio set
- RT kernel with 48 task, 49 TX/RX thread prio (or 49 prio for main thread if only one is used)
- RT kernel with JUST ethtool
- RT kernel with JUST tunedadm
- RT kernel with ethtool + tunedadm
- RT kernel with ethtool + prio
- RT kernel with tunedadm + prio
- RT kernel with ethtool + tunedadm + prio

## Test programs

For the 10 group tests, we don't need 10 devices - we can just send 10 empty LRW. Maybe not totally
representitive though. Maybe I can come up with _n_ devices which provide some PDI data. A pile of
IOs?

If only one task is used, use main thread. If more than one task, spawn in background and main
thread only joins them.

- 1 thread (tx/rx runs on this thread too), 1 group task in main loop
- 1 thread, 10 group tasks
- 2 threads, 1 group task, tx/rx runs in background thread
- 2 threads, 10 group tasks, tx/rx runs in background thread
- 11 threads, main thread just joins them all

## Cycle times

- 1000us (1ms)
- 100us (0.1ms) for a stress test

# Results

- Packet response time
  - Normal chart for display
  - Histogram
  - Stats: P95, P99, P25/P50 (median)/P75 (quartiles,) min/max, mean, standard deviation
- Application cycle time
  - Normal chart for display
  - Histogram
  - Stats: P95, P99, P25/P50 (median)/P75 (quartiles,) min/max, mean, standard deviation

# Side experiment: Taguchi table

See if using a Taguchi table/orthogonal array with whatever the most important result stat is. Maybe
mean? Standard deviation? Both would be interesting - both to see lower latency, and to see jitter.

Tables can be found
[here](https://www.me.psu.edu/cimbala/me345/Lectures/Taguchi_orthogonal_arrays.pdf).

# Setting Grub default kernel

Need to set strings in `/etc/default/grub` like:

```
gnulinux-advanced-8d15ba73-7f16-4fe9-8613-1aa61494e173>gnulinux-6.1.0-13-amd64-advanced-8d15ba73-7f16-4fe9-8613-1aa61494e173
```

Notice the `>` buried in there. Find the two segments with:

```bash
# First segment
sudo grep submenu /boot/grub/grub.cfg
# Second segment
sudo grep gnulinux /boot/grub/grub.cfg
```

Then update and reboot:

```bash
sudo vim /etc/default/grub
sudo update-grub
uname -a
```

Props to [here](https://www.gnu.org/software/grub/manual/grub/html_node/default.html#default) for
helping with this.
