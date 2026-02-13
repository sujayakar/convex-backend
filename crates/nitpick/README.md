# Nitpick

Deterministic simulation testing for the Convex storage engine. The spiritual
successor to `pedant`, operating at the Application layer with real JavaScript
UDF execution through V8 isolates.

## Quick Start

```bash
# Run all tests
cargo test -p nitpick

# Run a specific scenario with a seed (for debugging)
cargo run -p nitpick -- replay counter_js 42

# Run a batch of 100 simulations
cargo run -p nitpick -- batch counter_js -s 100

# List available scenarios
cargo run -p nitpick -- list
```

## Scenarios

Each scenario has two variants: a **database-level** version (raw Rust
transactions) and a **JS** version (real TypeScript UDFs through V8).

| Scenario  | DB             | JS                                 | What it tests                                                                   |
| --------- | -------------- | ---------------------------------- | ------------------------------------------------------------------------------- |
| counter   | `counter.rs`   | `counter_js.rs` + `counter.ts`     | Contended increment. Invariant: `triangle == (linear+1)*linear/2`               |
| elle      | `elle.rs`      | `elle_js.rs` + `elle.ts`           | Append-register with Elle-style dependency graph serializability check          |
| link_ring | `link_ring.rs` | `link_ring_js.rs` + `link_ring.ts` | Ring of linked documents. Each tx reverses a sub-chain. Invariant: single cycle |

## CLI

```
nitpick [OPTIONS] <COMMAND>

Options:
  -c, --concurrency <N>    Concurrent transactions per simulation [default: 5]
  -t, --transactions <N>   Transactions per simulation [default: 100]

Commands:
  replay <SCENARIO> <SEED>  Replay a specific simulation
  batch <SCENARIO>          Run many simulations with random seeds
    -s, --simulations <N>     Number of simulations [default: 1000]
    --threads <N>             Worker threads [default: 4]
  list                      List available scenarios
```

## Adding a New Scenario

### Database-level scenario

1. Create `crates/nitpick/src/scenarios/my_scenario.rs`
2. Implement the `Scenario` and `TestRun` traits
3. Add `pub mod my_scenario;` to `scenarios/mod.rs`
4. Add tests to `tests.rs`
5. Register in `main.rs`

### JS UDF scenario

1. Write TypeScript UDFs in `npm-packages/simulation/convex/my_scenario.ts`
2. Update the schema in `npm-packages/simulation/convex/schema.ts`
3. Rebuild the JS bundle:
   ```bash
   cd npm-packages/simulation
   npx convex-bundled deploy \
     --write-push-request dist/start_push \
     --url http://127.0.0.1:8000 \
     --admin-key $(cat ../../crates/keybroker/dev/admin_key.txt)
   ```
4. Create `crates/nitpick/src/scenarios/my_scenario_js.rs` using the
   `helpers::deploy_js`, `helpers::call_mutation`, `helpers::call_query` helpers
5. Add to `scenarios/mod.rs`, `tests.rs`, and `main.rs`

## Determinism

All simulations are deterministic given a seed. The `TestRuntime` provides:

- Seeded RNG (`ChaCha12Rng`)
- Virtualized time (starts from `CONVEX_EPOCH`)
- Single-threaded Tokio with paused timer

Determinism is verified by running the same (scenario, seed) twice and comparing
deterministic `TestResult` fields:

- `rng_next_u64`
- `output`
- trace event payload sequence (ignoring sequence numbers and elapsed wall-clock
  timing)

`num_polls` is still captured for diagnostics but is not used for equality. When
a determinism mismatch occurs, nitpick reports compact diagnostics including
each run's poll count and trace length. The diagnostics also include mismatch
flags (`rng_mismatch`, `output_mismatch`, `trace_len_mismatch`), which trace is
longer when lengths diverge, and first differing trace event index when
available (or the shorter trace boundary when only lengths differ).
`trace_mismatch_kind` indicates whether the trace mismatch comes from differing
event payloads or a length boundary. `trace_len_delta` and
`paired_trace_event_count` provide quick context for how far the traces diverge.
Output values are shown as debug previews and truncated when large (with full
debug lengths included), and diagnostics explicitly report whether previews were
truncated and the preview character limit. Poll counts remain diagnostic-only,
and `num_polls_delta` gives a quick signed difference between runs.

Why not include `num_polls` in equality? Tokio's `worker_poll_count` is a
scheduler metric that is flushed at scheduler submit points, and can vary
slightly under host CPU contention even when the logical test execution is
identical.

### Debugging nondeterminism under contention

When investigating flaky parallel determinism failures, prefer a single-seed
repro loop under external contention over large random-seed batches. This
reduces noise and helps isolate scheduler-sensitive behavior.

Example pattern:

```bash
# First, find a failing seed from a batch run.
cargo run --release -p nitpick -- -t 20 -c 4 batch index_query_js -s 1000 --threads 4

# Then hammer the same seed in parallel in separate processes.
python3 - <<'PY'
import concurrent.futures, subprocess
seed = "PUT_SEED_HERE"
cmd = ["target/release/nitpick", "-t", "20", "-c", "4", "replay", "index_query_js", seed]
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as ex:
    results = list(ex.map(lambda _: subprocess.run(cmd, cwd=".", capture_output=True).returncode, range(100)))
print("failures:", sum(code != 0 for code in results))
PY
```

## Code Coverage

```bash
# Run with coverage instrumentation
cargo llvm-cov -p nitpick

# Generate HTML report
cargo llvm-cov -p nitpick --html --output-dir target/coverage/html
open target/coverage/html/html/index.html
```

## Architecture

```
crates/nitpick/
  src/
    main.rs                    # CLI binary
    lib.rs                     # Crate root
    framework/
      scenario.rs              # Scenario + TestRun traits
      runner.rs                # run_once(), run_scenario(), determinism check
      batch.rs                 # Parallel batch execution
    scenarios/
      helpers.rs               # Shared: deploy_js(), call_mutation(), call_query()
      counter.rs / counter_js.rs
      elle.rs / elle_js.rs
      link_ring.rs / link_ring_js.rs
    tests.rs                   # All tests

npm-packages/simulation/convex/
  counter.ts                   # Counter UDFs
  elle.ts                      # Elle UDFs
  link_ring.ts                 # LinkRing UDFs
  schema.ts                    # Schema for all scenarios
```
