# Future: Structured Event Reporting

## Status: Future Phase (not in Phase 1)

## Problem

Phase 1 collects event logs specific to each scenario (e.g., Elle model
events for serializability checking). There is no unified, structured event
stream that captures everything happening inside the system during a
simulation run.

A structured event stream would enable:

- Post-hoc analysis by judges (exact and nuanced)
- Execution trace visualization and debugging
- Coverage analysis (which code paths were exercised)
- LLM-based exploration (the LLM reads the trace to suggest actions)
- Failure minimization (replay subsets of the trace)

## Current State

- `PauseClient` breakpoints exist at 20+ points in the codebase but are
  binary (pause/resume) -- they don't emit structured events.
- `tracing` spans exist throughout the codebase but are unstructured text
  aimed at human debugging, not machine analysis.
- Elle model events (`ElleModelEvent`) are scenario-specific, not generic.
- Pedant's `PedantDatabase` logs operations via `slog_trace!` but this is
  unstructured.

## Approach

### TraceRecorder

Add an optional `TraceRecorder` to the `Runtime` trait (or alongside
`PauseClient`):

```rust
trait Runtime {
    // existing...
    fn trace_recorder(&self) -> TraceRecorder;
}
```

`TraceRecorder` is a no-op in production. In simulation, it records
structured events to a thread-safe append-only log:

```rust
struct TraceEvent {
    timestamp: SimulatedTime,
    wall_clock: Duration,
    source: EventSource,
    event: Event,
}

enum Event {
    TransactionBegin { tx_id: TxId, identity: Identity },
    TransactionCommit { tx_id: TxId, timestamp: Timestamp },
    TransactionConflict { tx_id: TxId },
    UdfExecutionStart { function: FunctionPath, args_hash: u64 },
    UdfExecutionEnd { function: FunctionPath, duration: Duration },
    SubscriptionCreated { token: Token },
    SubscriptionInvalidated { token: Token },
    WorkerTick { worker: WorkerKind },
    PausePointHit { label: &'static str },
    FaultInjected { label: &'static str, fault: FaultKind },
    Custom { label: String, data: serde_json::Value },
}
```

### Instrumentation Points

Add `trace_recorder().record(...)` calls at the same locations where
`pause_client().wait(...)` calls already exist, plus additional points:

- Transaction begin/commit/conflict in `Database`
- UDF execution start/end in `IsolateClient`
- Subscription creation/invalidation in `Database`
- Worker ticks in index/search/schema workers
- Scheduled job execution in `ScheduledJobRunner`

### Serialization

The `ExecutionTrace` (ordered list of `TraceEvent`s) should be
serializable to JSON/MessagePack for:

- Storage alongside rollout results
- Input to LLM analysis
- Visualization tools
- Replay debugging

## Key Design Questions

- Should `TraceRecorder` be on the `Runtime` trait or passed separately?
  (Following the `PauseClient` precedent, it could be on `Runtime`.)
- How much overhead is acceptable for recording? (Must be cheap enough
  to not distort simulation timing.)
- What is the right granularity? Too fine-grained produces huge traces;
  too coarse misses important events.
- Should events be typed (enum) or stringly-typed (for extensibility)?

## Dependencies

- Phase 1 DST framework (to provide the execution context)
- Useful for: LLM exploration, failure minimization, instrumentation
