# Nitpick — Cloud Agent Build & Test Instructions

## Cloud-specific build setup

The nitpick crate has heavy native dependencies (V8, RocksDB, aws-lc-sys) and
requires a JavaScript build toolchain (Rush) for the isolate crate's build
script. Follow these steps in order.

### 1. Fix the C++ toolchain

The cloud VM ships with clang-18 as `c++` but it selects gcc-14 headers that
aren't installed. Force gcc as the C++ compiler:

```bash
export CXX=g++
export CC=gcc
```

Set these **before every `cargo build` / `cargo check`** invocation (they don't
persist across shell sessions in the agent environment).

If you see errors about `libstdc++` not found during linking, create the
symlink:

```bash
sudo ln -s /usr/lib/gcc/x86_64-linux-gnu/13/libstdc++.so \
           /usr/lib/x86_64-linux-gnu/libstdc++.so
```

### 2. Install Rush (JavaScript monorepo build tool)

The `isolate` crate's `build.rs` invokes Rush to build JS packages
(`npm-packages/simulation`, `npm-packages/udf-runtime`, etc.). Rush must be
available at `scripts/node_modules/.bin/rush`:

```bash
cd /workspace/scripts && npm install
```

This is a one-time step. If you see `"Failed on rush install"` from `cargo
build`, this is what's missing.

### 3. Unset RUSTUP_TOOLCHAIN

The repo uses a `rust-toolchain` file (nightly). If `RUSTUP_TOOLCHAIN` is set
in the environment it overrides this. Always unset it:

```bash
unset RUSTUP_TOOLCHAIN
```

### 4. Build commands

```bash
# Check (faster, no linking):
cargo check -p nitpick

# Debug build:
cargo build -p nitpick

# Release build (required for batch testing — debug is too slow):
cargo build --release -p nitpick

# Check tests compile:
cargo check -p nitpick --tests
```

The first build takes 5–10 minutes (V8 + RocksDB + aws-lc-sys compilation).
Incremental rebuilds of just the nitpick crate take ~4 minutes in release mode.

### 5. One-liner for a clean build

```bash
cd /workspace && export CXX=g++ && export CC=gcc && unset RUSTUP_TOOLCHAIN && cargo build --release -p nitpick
```

## Running nitpick

```bash
# Replay a single seed (10% chance of determinism check):
./target/release/nitpick -t 20 -c 4 replay index_query_js <SEED>

# Batch mode (random seeds, parallel):
./target/release/nitpick -t 20 -c 4 batch index_query_js -s 1000 --threads 4

# List available scenarios:
./target/release/nitpick list
```

### Controlling the determinism check probability

In `crates/nitpick/src/framework/runner.rs`, the constant
`DETERMINISM_CHECK_PROBABILITY` (default `0.1`) controls how often `run_scenario`
re-runs with the same seed to verify determinism. Set it to `1.0` for debugging
to guarantee the check fires every time.

## Testing determinism

The key metric is `worker_poll_count(0)` from Tokio's runtime metrics, measured
at the end of `run_transactions` in `runner.rs`. A determinism failure means
the same seed produced different `num_polls`, `rng_next_u64`, or `output`
between two sequential runs in the same process.

**Single-seed replay** is the cheapest way to test — it runs one simulation and
(probabilistically) checks determinism with no external contention.

**Batch mode** is the stress test — multiple simulations share the same process
and V8 platform, exposing cross-simulation state leakage.
