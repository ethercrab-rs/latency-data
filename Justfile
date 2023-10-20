run *args:
    # Clean up previous failed runs
    killall tshark || true

    cargo build --release
    sudo echo
    fd . --type executable ./target/debug -x sudo setcap cap_net_raw=pe
    fd . --type executable ./target/release -x sudo setcap cap_net_raw=pe
    cargo run --release -- {{args}}
