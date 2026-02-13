# Future: Network Simulation in the Runtime Trait

## Status: Future Phase (not in Phase 1)

## Problem

Phase 1 uses the existing fault injection mechanisms:

- `JsClientThread::disconnect_network()` / `reconnect_network()` for
  client-level partitions
- `ServerThread`'s `expected_delay_duration` for geometric delay on sync
  messages
- `PauseClient` breakpoints for pausing at specific code points

These are sufficient for basic interleaving exploration but don't support:

- Fine-grained network partitions between specific components
- Message reordering
- Packet loss (probabilistic)
- Bandwidth throttling
- Split-brain scenarios
- Asymmetric partitions (A can reach B but not vice versa)

## Current State

The `Runtime` trait abstracts time, spawning, RNG, and pause but has **no
network abstraction**. Networking happens outside the trait:

- `ServerThread` uses `tokio::sync::mpsc` channels
- `JsClientThread` uses channels to `ServerThread`
- `Application` takes trait objects for `Persistence`, `Storage`, `FetchClient`,
  `Searcher` -- these could be wrapped for simulation

## Approach

### Option A: Wrap Trait Boundaries (Sim-prefixed wrappers)

Create simulation wrappers around the existing trait objects that `Application`
depends on:

```rust
struct SimPersistence {
    inner: TestPersistence,
    network: SimNetwork,
    node_id: NodeId,
}

impl Persistence for SimPersistence {
    async fn write(...) -> Result<...> {
        self.network.simulate_latency(self.node_id, "persistence").await?;
        self.inner.write(...).await
    }
}
```

Similarly for `Storage`, `FetchClient`, `Searcher`.

Pros: No changes to `Runtime` trait. Works with existing code. Cons: Only
intercepts at trait boundaries, not arbitrary network calls.

### Option B: Add Network to Runtime Trait

Add a `Network` abstraction to the `Runtime` trait:

```rust
trait Runtime {
    // existing...
    fn network(&self) -> &dyn NetworkSimulator;
}

trait NetworkSimulator {
    async fn send(&self, from: NodeId, to: NodeId, msg: Bytes) -> Result<()>;
    async fn recv(&self, node: NodeId) -> Result<Bytes>;
    fn partition(&self, a: NodeId, b: NodeId);
    fn heal(&self, a: NodeId, b: NodeId);
}
```

Pros: Comprehensive control. All network I/O goes through the simulator. Cons:
Large refactor. Production `Runtime` would need a pass-through impl.

### Option C: SimNetwork as Middleware on Channels

Wrap the `tokio::sync::mpsc` channels used by `ServerThread` and
`JsClientThread` with a `SimNetwork` layer:

```rust
struct SimNetwork {
    rng: Box<dyn RngCore>,
    partitions: HashSet<(NodeId, NodeId)>,
    delay_distribution: DelayDistribution,
    message_queue: BTreeMap<Instant, (NodeId, Message)>,
}
```

Pros: Minimal changes. Only affects the simulation crate. Cons: Only covers
client-server communication, not persistence/storage.

### Recommendation

Start with Option C (channel middleware) for client-server communication, then
add Option A wrappers for persistence/storage as needed. Option B is a larger
refactor best deferred until the simpler approaches are proven insufficient.

## SimNetwork Capabilities

- **Delay**: Configurable distribution (exponential, uniform, constant)
- **Partition**: Block messages between node pairs
- **Reorder**: Buffer messages and deliver out of order
- **Drop**: Probabilistic message loss
- **Asymmetric**: Different rules for A->B vs B->A
- **Bandwidth**: Rate-limit message throughput

## Key Design Questions

- Should network simulation be per-rollout configurable or per-action?
- How to handle in-flight messages during partition events
- Whether to model TCP semantics (ordered delivery within connection) or UDP
  semantics (unordered)
- How to make network events appear in the structured event trace

## Dependencies

- Phase 1 DST framework
- Structured event reporting (for tracing network events)
