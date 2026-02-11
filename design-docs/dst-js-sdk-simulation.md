# JavaScript SDK Client Simulation

## Status: Proof of Concept (implemented)

## Problem

The existing `JsClientThread` in `crates/nitpick/src/simulation/js_client/`
wraps `BaseConvexClient` (the lower-level SDK client) in a V8 isolate with
simulated WebSocket channels. While this exercises the core sync protocol,
it doesn't cover the higher-level `ConvexClient` API code paths:
callback-based subscription management (`onUpdate`), Promise-based mutation
execution, `setTimeout` scheduling, connection state tracking, etc.

We want to run the **real ConvexClient** (from `npm-packages/convex/`)
inside the deterministic simulation, so that higher-level client-side bugs
are exercisable and discoverable by the DST framework.

## Prior Art

- The existing `JsClientThread` in `crates/nitpick/src/simulation/js_client/`
  wraps `BaseConvexClient` via the simulation bundle at
  `npm-packages/simulation/src/main.ts`. It handles queries, mutations,
  subscriptions, and network disconnect/reconnect.
- The `npm-packages/convex/` directory contains the real JS SDK, including
  both `BaseConvexClient` (in `browser/sync/client.ts`) and `ConvexClient`
  (in `browser/simple_client.ts`).
- V8 isolates are already used in the backend for UDF execution, and they are
  deterministic under `TestRuntime` (seeded RNG, frozen time).

## Implementation

### Architecture

The SDK client simulation follows the same architecture as `JsClientThread`
but wraps `ConvexClient` instead of `BaseConvexClient`:

```
┌─────────────────────────────────────┐
│          DST Scenario               │
│  (sdk_client_poc.rs)                │
│  subscribe / run_mutation / ...     │
└───────────┬─────────────────────────┘
            │ SdkClientRequest
            ▼
┌─────────────────────────────────────┐
│        SdkClientThread (Rust)       │
│  crates/nitpick/src/simulation/     │
│  sdk_client/                        │
│  ┌───────────────────────────────┐  │
│  │  V8 Isolate                   │  │
│  │  ┌─────────────────────────┐  │  │
│  │  │  ConvexClient (JS SDK)  │  │  │
│  │  │  ┌───────────────────┐  │  │  │
│  │  │  │ BaseConvexClient  │  │  │  │
│  │  │  │ WebSocketManager  │  │  │  │
│  │  │  └─────┬─────────────┘  │  │  │
│  │  │  SdkTestingWebSocket    │  │  │
│  │  └────────┬────────────────┘  │  │
│  └───────────┼───────────────────┘  │
│              │ outgoingMessages /    │
│              │ receiveIncomingMessages│
│              ▼                       │
│  ┌───────────────────────────────┐  │
│  │  WebSocket Bridging           │  │
│  │  (channels to ServerThread)   │  │
│  └───────────────────────────────┘  │
└───────────┬─────────────────────────┘
            │ mpsc channels
            ▼
┌─────────────────────────────────────┐
│        ServerThread                 │
│  (SyncWorker + ApplicationApi)      │
└─────────────────────────────────────┘
```

### Components

1. **JavaScript Wrapper** (`npm-packages/simulation/src/sdk_client.ts`):
   - Self-contained module (no imports from `websocket.ts` or `protocol.ts`
     to avoid module-level side effects that would create a second
     `BaseConvexClient`).
   - Inlines `SdkTestingWebSocket` class and the outgoing/incoming message
     protocol.
   - Creates a `ConvexClient` with `SdkTestingWebSocket` as the WebSocket
     constructor.
   - Exposes functions: `subscribe`, `getSubscriptionResult`, `unsubscribe`,
     `runMutation`, `getCompletedMutations`, `getSdkMaxObservedTimestamp`.
   - Built via esbuild to `dist/sdk_client.js`.

2. **Rust Module** (`crates/nitpick/src/simulation/sdk_client/`):
   - `mod.rs`: `SdkClientThread` struct with public API (subscribe,
     get_subscription_result, unsubscribe, run_mutation, etc.)
   - `go.rs`: Main event loop (process inbox/outbox, microtask checkpoint,
     select on requests/WebSocket/timers)
   - `state.rs`: V8 function handles, WebSocket bridging, network state
   - `environment.rs`: `SdkClientEnvironment` with TestRuntime for
     deterministic time/RNG/timers
   - `protocol.rs`: Serde types for JS function arguments/results

3. **Test Scenario** (`crates/nitpick/src/scenarios/sdk_client_poc.rs`):
   - Deploys JS UDFs (subscription counter)
   - Creates `SdkClientThread` + `ServerThread`
   - Subscribes to counter query via `ConvexClient.onUpdate`
   - Runs mutations that increment the counter
   - Verifies subscription converges to correct value

### Key Design Decisions

- **Self-contained JS module**: The `sdk_client.ts` bundle does not import
  from `websocket.ts` or `protocol.ts` because those modules create a global
  `BaseConvexClient` as a side effect. Instead, `SdkTestingWebSocket` and
  the message protocol are inlined.

- **WebSocket shimming**: `SdkTestingWebSocket` records connect/send/close
  events in an `outgoingMessages` array. Rust drains this array via
  `getOutgoingMessages()` and routes messages through channels to
  `ServerThread`. Server responses are fed back via
  `receiveIncomingMessages()`.

- **Timer handling**: `ConvexClient` uses `setTimeout` internally (e.g., for
  `callNewListenersWithCurrentValues`). These are handled by
  `SdkClientEnvironment.start_async_op(Sleep { .. })` which creates tokio
  timers that resolve in the main loop's `tokio::select!`.

- **Deterministic execution**: Uses `TestRuntime`'s seeded RNG
  (`ChaCha12Rng`) and virtualized `unix_timestamp()`. Combined with the
  single-threaded deterministic Tokio runtime, execution is fully
  reproducible.

## What This Exercises (vs. JsClientThread)

| Feature                          | JsClientThread | SdkClientThread |
|----------------------------------|:--------------:|:---------------:|
| `BaseConvexClient` sync protocol | ✓              | ✓               |
| `ConvexClient.onUpdate` callbacks| ✗              | ✓               |
| Promise-based mutation results   | ✗              | ✓               |
| `setTimeout` scheduling          | ✓ (basic)      | ✓ (SDK-level)   |
| Connection state tracking        | ✗              | ✓               |
| Network disconnect/reconnect     | ✓              | ✓               |
| Deterministic RNG/time           | ✓              | ✓               |

## Future Work

- **Paginated queries**: Exercise `ConvexClient.onPaginatedUpdate_experimental`.
- **Auth token handling**: Test `setAuth`/`clearAuth` with simulated token
  refresh.
- **Action execution**: Exercise `ConvexClient.action()`.
- **Optimistic updates**: Test `MutationOptions.optimisticUpdate`.
- **Reconnection stress tests**: Rapid disconnect/reconnect cycles.
- **Multiple clients**: Multiple `SdkClientThread` instances subscribing to
  overlapping queries.
- **Determinism verification**: Run the SDK client PoC scenario with
  `run_scenario_deterministic` to verify full reproducibility.

## Dependencies

- Phase 1 DST framework (Scenario trait, Runner, BatchRunner)
- Network simulation in Runtime trait (see `dst-network-simulation.md`)
