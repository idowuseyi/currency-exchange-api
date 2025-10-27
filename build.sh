#!/bin/bash
set -e  # Exit on any error

# Update package list
apt-get update

# Install fontconfig and pkg-config for plotters
apt-get install -y libfontconfig1-dev pkg-config

# Build the Rust binary
cargo build --release

# Optional: Set the binary as executable and log success
chmod +x target/release/currency-exchange-api
echo "Build complete: Binary at target/release/currency-exchange-api"
