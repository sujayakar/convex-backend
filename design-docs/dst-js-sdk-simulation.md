# Future: JavaScript SDK Client Simulation

## Status: Future Phase (not in Phase 1)

## Problem

Phase 1 simulates JS clients via `JsClientThread`, which is a Rust-side
reimplementation of client sync behavior. This misses bugs that live in the
actual JavaScript SDK code (subscription management, optimistic updates,
reconnection logic, auth token handling, etc.).

We want to run **real JavaScript SDK client code** inside the deterministic
simulation, so that client-side bugs are exercisable and discoverable by the DST
framework.

## Prior Art

- The existing `JsClientThread` in
  `crates/simulation/src/test_helpers/js_client/` reimplements client protocol
  logic in Rust. It handles queries, mutations, subscriptions, and network
  disconnect/reconnect -- but it's a separate implementation from the real JS
  SDK.
- The `npm-packages/convex/` directory contains the real JS SDK.
- V8 isolates are already used in the backend for UDF execution, and they are
  deterministic under `TestRuntime` (seeded RNG, frozen time).

## Approach

### Option A: Run JS SDK in a V8 Isolate

Embed the real JS SDK in a V8 isolate within the simulation. This isolate would:

1. Load the bundled JS SDK code
2. Expose a simulated `WebSocket` / `fetch` API that routes through the
   `SimNetwork` (channels to `ServerThread`) instead of real networking
3. Use the `TestRuntime`'s seeded RNG and virtualized time for `Math.random()`
   and `Date.now()`
4. Be driven by the DST runner -- the runner calls into the isolate to trigger
   client actions (subscribe to query, run mutation, etc.)

Key challenge: the JS SDK uses `setTimeout`, `setInterval`, `Promise`
scheduling, and `WebSocket` event handlers. These need to be wired to the
deterministic Tokio runtime so that timer advancement and event delivery are
controlled by the simulation.

### Option B: Compile JS SDK to WASM

Compile the JS SDK to WASM and run it inside the Rust simulation directly. This
gives full control over the execution environment but requires significant build
tooling work.

### Recommendation

Option A is more practical given the existing V8 infrastructure. The isolate
execution is already deterministic, and the main work is shimming the network
APIs.

## Key Design Questions

- How to handle `WebSocket` in a simulated isolate (event-driven vs polling)
- How to synchronize JS event loop ticks with the deterministic Tokio runtime
- Whether to run one V8 isolate per simulated client or multiplex
- How to handle the SDK's reconnection/backoff logic under virtualized time

## Dependencies

- Phase 1 DST framework (Scenario trait, Runner, BatchRunner)
- Network simulation in Runtime trait (see `dst-network-simulation.md`)
