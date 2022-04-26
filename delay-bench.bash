#!/usr/bin/bash

set -eEuxo pipefail

trap "sudo tc qdisc del dev lo root" SIGINT SIGTERM ERR EXIT

sudo tc qdisc add dev lo root netem delay 10ms loss 10%
cargo bench
