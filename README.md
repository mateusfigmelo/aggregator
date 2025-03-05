# GTE/Liquid Labs Takehome Challenge

## Overview

This is a takehome challenge for GTE/Liquid Labs inspired by real problems our engineering team has faced.

## Setup Instructions

1. Install Rust and Cargo: [Rust Installation Guide](https://www.rust-lang.org/tools/install)
2. Clone the repository: `git clone https://github.com/gte-liquid-labs/takehome-challenge.git`

## Running the Code

This code has been written as a CLI. To run the aggregator, you can use the following command:

```bash
RUST_LOG=aggregator=info cargo run --release -p aggregator -- run
```

Where everything past the '--' are CLI flags as defined in `packages/aggregator/src/cli/entry.rs`.

There are a few (optional) flags you can pass to the CLI:

- `--num-tokens`: The number of tokens that exist for the simulation. Defaults to 10.
- `--events-per-second`: The number of orderbook events to process per second. Defaults to 100.
- `--requests-per-second`: The number of swap requests to process per second. Defaults to 100.

## Directory Structure

This code has been organized into a monorepo with the following structure:

- `packages/aggregator/src/takehome`: The takehome implementation of the `EventProcessor` and `RequestProcessor` traits. Most of your work will be done here.
- `packages/aggregator/src/backend`: The background threads that simulate incoming events and requests. You should not need to modify this.
- `packages/aggregator/src/cli`: The CLI implementation. You should only modify this to add any additional flags.
- `packages/aggregator-utils/src/types`: The types used by the aggregator. Feel free to add any convenience functions here.
