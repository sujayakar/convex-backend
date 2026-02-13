# Future: Failure Minimization and Test Distillation

## Status: Future Phase (not in Phase 1)

## Problem

When the DST framework finds a failure (serializability violation, invariant
breach, panic), the failing rollout may involve hundreds of transactions across
multiple clients with many fault injection points. Understanding _why_ the
failure occurred requires distilling the trace down to the minimal set of
actions that reproduces the bug.

## Goals

1. **Minimize**: Given a failing (scenario, seed, config), find the smallest
   config (fewest transactions, lowest concurrency, fewest fault injections)
   that still reproduces the failure.

2. **Distill**: Produce a standalone, deterministic test case that can be
   checked into the repo as a regression test.

3. **Explain**: Produce a human-readable explanation of what went wrong.

## Approach

### Delta Debugging (Automated)

Classic delta debugging adapted for action sequences:

1. Start with the full action sequence `A` that causes failure
2. Binary search: try first half, second half
3. If a half still fails, recurse on that half
4. If neither half fails, try removing individual actions
5. Result: minimal subset `A'` that still reproduces the failure

Implementation:

```rust
struct Minimizer {
    scenario: Box<dyn Scenario>,
    seed: u64,
    judge: Box<dyn Judge>,  // the judge that detected the failure
}

impl Minimizer {
    /// Returns the minimal action sequence that reproduces the failure.
    async fn minimize(&self, actions: Vec<Action>) -> Vec<Action> {
        // Delta debugging loop
        // Each iteration re-runs the scenario with a subset of actions
        // and checks if the judge still reports failure
    }
}
```

Key insight: since rollouts are deterministic, we can replay with modified
action sequences and get reproducible results.

### Configuration Minimization

Before minimizing the action sequence, try reducing the configuration:

1. Reduce `concurrency` (e.g., from 50 to 2)
2. Reduce `transactions` (e.g., from 500 to the minimum that fails)
3. Disable fault injection categories one by one

This is cheaper than action-level delta debugging and often sufficient.

### LLM-Assisted Distillation

After automated minimization, use an LLM to:

1. Read the minimized trace
2. Identify the essential causal chain
3. Generate a standalone test function that:
   - Sets up the scenario
   - Executes only the necessary actions in the right order
   - Asserts the invariant that was violated

```rust
#[async_trait]
trait Distiller: Send + Sync {
    async fn distill(
        &self,
        scenario: &dyn Scenario,
        minimized_trace: &ExecutionTrace,
        failure: &Finding,
    ) -> Result<String>;  // Generated Rust test code
}
```

### Output Format

A minimized failure should produce:

```
=== FAILURE FOUND ===
Scenario: counter
Seed: 12345678
Transactions: 5 (minimized from 500)
Concurrency: 2 (minimized from 50)
Judge: SerializabilityJudge

Replay: cargo run -- replay counter 12345678 -c 2 -t 5

Minimized trace:
  1. Client 0: mutation counter:increment
  2. Client 1: mutation counter:increment  (concurrent)
  3. [Fault: after_pending_write_snapshot paused]
  4. Client 0: mutation counter:increment
  5. Client 1: query counter:get -> invariant violated

Generated test: tests/regression/counter_12345678.rs
```

## Key Design Questions

- How to handle non-determinism in minimization (some action removals may change
  the interleaving in ways that mask or reveal the bug)
- Whether to minimize at the action level or the seed level (try nearby seeds
  that also fail, find the simplest one)
- How to handle scenarios where the failure requires specific timing (fault
  injection at a specific breakpoint)
- Cost of re-running rollouts during minimization (each is a full Application
  startup)

## Dependencies

- Phase 1 DST framework (deterministic replay is essential)
- Structured event reporting (for trace analysis)
- LLM integration (for distillation, optional for minimization)
