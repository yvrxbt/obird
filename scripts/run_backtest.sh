#!/bin/bash
set -euo pipefail
CONFIG=${1:-configs/example.toml}
DATA=${2:-data/recordings/latest}
OUTPUT=${3:-data/backtest_results/$(date +%Y%m%d_%H%M%S)}
echo "Running backtest: Config=$CONFIG Data=$DATA Output=$OUTPUT"
mkdir -p "$OUTPUT"
RUST_LOG=info cargo run --release -- backtest --config "$CONFIG" --data "$DATA" --output "$OUTPUT"
