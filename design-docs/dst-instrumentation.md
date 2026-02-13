# Future: LLM-Guided Instrumentation and Coverage-Guided Exploration

## Status: Future Phase (not in Phase 1)

## Problem

The DST framework explores the system's state space by randomly choosing actions
and injecting faults. This is effective but can miss deep bugs that require
specific sequences of actions to reach. Two complementary techniques can improve
exploration:

1. **Coverage-guided exploration**: Track which code paths have been exercised
   and bias exploration toward unexercised paths (similar to AFL/libFuzzer for
   fuzzing).

2. **LLM-guided instrumentation**: An LLM reads the codebase and execution
   traces, then suggests adding instrumentation points to gain observability
   into specific subsystems. The harness modifies the source code, recompiles,
   and re-runs with richer traces.

## Coverage-Guided Exploration

### Approach

Use Rust's built-in coverage instrumentation (`-C instrument-coverage`) to
collect branch/line coverage during simulation runs:

```rust
struct GuidedExplorer {
    rng: Box<dyn RngCore>,
    coverage_map: CoverageMap,
    /// Favor actions that have historically led to new coverage
    action_scores: HashMap<ActionKind, f64>,
}

impl GuidedExplorer {
    fn update_scores(&mut self, actions: &[Action], new_coverage: &CoverageMap) {
        for action in actions {
            if new_coverage.has_new_branches() {
                *self.action_scores.entry(action.kind()).or_default() += 1.0;
            }
        }
    }
}
```

### Coverage Integration

1. Compile the simulation binary with `-C instrument-coverage`
2. After each rollout, read the coverage data (`.profraw` files)
3. Merge into a cumulative coverage map
4. Use the coverage map to weight action selection in subsequent rollouts
5. Report overall coverage achieved across all rollouts

This is similar to what pedant's README describes with `grcov`, but integrated
into the exploration loop rather than as a post-hoc report.

### Breakpoint Coverage

In addition to code coverage, track which `PauseClient` breakpoints were hit
during each rollout. This provides a coarser but more meaningful coverage metric
for the DST framework:

```rust
struct BreakpointCoverage {
    /// Which breakpoints were hit at least once
    hit: HashSet<&'static str>,
    /// Which breakpoints had faults injected
    faulted: HashSet<&'static str>,
    /// Total breakpoint hits per label
    hit_counts: HashMap<&'static str, usize>,
}
```

## LLM-Guided Instrumentation

### Vision

The most novel aspect of the whitebox DST framework. The harness can **modify
source code** to add instrumentation, recompile, and re-run:

```
1. Run rollout with baseline instrumentation
2. LLM analyzes trace + source code
3. LLM suggests new instrumentation points
4. Harness modifies source code (adds observe() calls)
5. Recompile
6. Re-run with same seed → richer trace
7. LLM analyzes richer trace
8. Repeat until budget exhausted or finding distilled
```

### InstrumentationClient

A lightweight observation mechanism, following the `PauseClient` pattern:

```rust
/// In production: no-op. In simulation: records to trace.
struct InstrumentationClient {
    recorder: Option<TraceRecorder>,
}

impl InstrumentationClient {
    pub fn observe(&self, label: &'static str, data: impl Serialize) {
        if let Some(recorder) = &self.recorder {
            recorder.record(Event::InstrumentationPoint {
                label: label.to_string(),
                data: serde_json::to_value(data).unwrap_or_default(),
            });
        }
    }
}
```

Add to the `Runtime` trait alongside `PauseClient`:

```rust
trait Runtime {
    // existing...
    fn instrumentation_client(&self) -> InstrumentationClient;
}
```

### Code Modification

The `Instrumenter` suggests code modifications:

```rust
#[async_trait]
trait Instrumenter: Send + Sync {
    async fn suggest(
        &self,
        rollout: &Rollout,
        source_files: &SourceIndex,
    ) -> Result<Vec<CodeModification>>;
}

struct CodeModification {
    file_path: PathBuf,
    line: usize,
    kind: ModificationKind,
}

enum ModificationKind {
    /// Insert an observe() call before or after the line
    InsertObserve { label: String, expression: String },
    /// Insert a conditional breakpoint
    InsertBreakpoint { label: String, condition: String },
}
```

### Safety Constraints

- Only `instrumentation_client.observe(...)` calls can be inserted
- All modifications are tracked and reversible (git stash or patch files)
- A compilation check (`cargo check`) runs before re-executing
- Modifications are scoped: no changes to production code paths
- Budget limits on number of recompilation cycles

### Recompilation Loop

The recompilation loop is expensive (full `cargo build`). Mitigations:

1. Use incremental compilation (only modified files recompile)
2. Limit to a small number of iterations (3-5)
3. Batch multiple instrumentation suggestions per cycle
4. Cache compiled binaries keyed by instrumentation set

## Key Design Questions

- How to make coverage collection fast enough for the inner loop (coverage
  instrumentation adds ~10-20% overhead)
- Whether to use source-level or LLVM-IR-level coverage
- How to scope LLM instrumentation suggestions (which files to show the LLM, how
  much context)
- How to handle the recompilation latency (minutes per cycle)
- Whether instrumentation should persist across rollouts or be per-investigation

## Dependencies

- Phase 1 DST framework
- Structured event reporting (for TraceRecorder)
- LLM integration (for Instrumenter)
