# Future: LLM-Based Exploration, Judges, and Scoring

## Status: Future Phase (not in Phase 1)

## Problem

Phase 1 uses random exploration (weighted action sampling from a seeded
RNG) and exact judges (Elle serializability, algebraic invariants like
the counter test). This is effective for finding known classes of bugs
but has limitations:

- Random exploration may miss rare interleavings that require specific
  action sequences
- Exact judges can only check properties that can be formally specified
- No mechanism to discover *new* categories of bugs or suspicious behavior
- No way to prioritize which rollouts are most interesting

## Vision

Use LLMs as a **pluggable intelligence layer** on top of the DST
framework:

1. **LLM Explorer**: An LLM periodically analyzes the execution trace
   and suggests which actions to take next, biasing exploration toward
   interesting states.

2. **LLM Code Generator**: An LLM **synthesizes new test code** --
   mutations, queries, and actions -- that probe the system in ways
   the scenario author didn't anticipate. This goes beyond choosing
   from a fixed action space: the LLM can write entirely new JS UDFs
   that exercise edge cases, unusual argument patterns, schema
   violations, race-prone access patterns, etc.

3. **LLM Judge**: An LLM analyzes completed rollouts and scores them
   for suspicious patterns, potential bugs, or interesting behavior
   that formal judges can't capture.

4. **LLM Analyst**: When an exact judge finds a failure, an LLM
   analyzes the trace to explain the root cause in human-readable terms.

## Architecture

```
Explorer (generates actions)
  ├── RandomExplorer (baseline, seeded RNG)
  ├── GuidedExplorer (coverage-guided)
  └── LLMExplorer (LLM-assisted)
        ├── inner: RandomExplorer (fallback)
        ├── llm_client: Box<dyn LLMClient>
        ├── budget: ComputeBudget
        └── consultation_interval: usize

Judge (scores rollouts)
  ├── ExactJudge (binary pass/fail)
  │     ├── SerializabilityJudge (Elle)
  │     └── InvariantJudge (scenario-specific)
  └── NuancedJudge (numeric score)
        ├── PerformanceJudge (latency/throughput)
        ├── FairnessJudge (client starvation)
        └── LLMJudge (pattern recognition)
```

### LLMClient Trait

```rust
#[async_trait]
trait LLMClient: Send + Sync {
    async fn suggest_actions(
        &self,
        trace_summary: &str,
        action_space: &ActionSpace,
        findings_so_far: &[Finding],
    ) -> Result<Vec<SuggestedAction>>;

    /// Generate new JS UDF source code (mutations, queries, actions)
    /// that will be deployed and executed in subsequent rollouts.
    async fn generate_test_code(
        &self,
        schema: &str,
        existing_udfs: &[UdfSource],
        trace_summary: &str,
        findings_so_far: &[Finding],
    ) -> Result<GeneratedTestCode>;

    async fn analyze_failure(
        &self,
        trace: &ExecutionTrace,
        failure: &RolloutOutcome,
    ) -> Result<FailureAnalysis>;

    async fn score_rollout(
        &self,
        trace: &ExecutionTrace,
        scenario_description: &str,
    ) -> Result<NuancedScore>;
}
```

### LLM Code Generation

The most powerful mode: the LLM writes **new JavaScript UDFs** that
get deployed into the simulated Application and executed in rollouts.

```rust
struct GeneratedTestCode {
    /// New mutations/queries/actions to deploy
    udfs: Vec<GeneratedUdf>,
    /// How to invoke them (action templates for the explorer)
    action_templates: Vec<ActionTemplate>,
    /// Invariants the LLM expects to hold
    expected_invariants: Vec<String>,
}

struct GeneratedUdf {
    /// e.g., "llm_generated/racyCounter"
    function_path: String,
    /// The JS/TS source code
    source: String,
    /// mutation | query | action
    udf_type: UdfType,
}
```

**How it works:**

1. The LLM receives the schema, existing UDFs, and a summary of
   what's been tested so far.
2. It generates new UDFs designed to probe edge cases:
   - Mutations that create specific race conditions (e.g., read-modify-write
     on the same document from different paths)
   - Queries that test snapshot isolation under concurrent writes
   - Actions that combine mutations with external calls in tricky orderings
   - UDFs with unusual argument patterns, large documents, nested
     transactions, etc.
3. The generated code is bundled into a `StartPushRequest`, deployed
   via `Application::run_test_push()`, and added to the scenario's
   action space.
4. Subsequent rollouts can invoke the generated UDFs alongside the
   original scenario's UDFs.

**Validation loop:**

```
1. LLM reads schema + existing UDFs + trace summaries
2. LLM generates new UDFs + expected invariants
3. Harness deploys generated UDFs into the Application
4. Harness runs rollouts with the expanded action space
5. Judges check both existing and LLM-specified invariants
6. Results feed back to LLM for the next generation cycle
```

**Safety:**

- Generated UDFs run in the same V8 sandbox as normal UDFs (no
  filesystem/network access beyond what Convex allows)
- The harness validates that generated code compiles (via the push
  pipeline) before running rollouts
- Budget limits on how many generation cycles are allowed
- Generated UDFs are logged for human review

**What makes this powerful:**

Unlike traditional fuzzing (which mutates bytes) or property testing
(which generates data), LLM code generation operates at the
**semantic** level. The LLM understands Convex's transaction model,
can reason about race conditions, and can write UDFs that specifically
target subtle consistency issues. It's essentially an automated
adversarial red-teamer for the database.

### Compute Budget

LLM calls are expensive and non-deterministic. The framework enforces
a budget:

```rust
struct ComputeBudget {
    max_tokens: usize,
    max_calls: usize,
    max_cost_usd: f64,
    tokens_used: usize,
    calls_made: usize,
}
```

When the budget is exhausted, the `LLMExplorer` falls back to its
inner `RandomExplorer`.

### Determinism Considerations

LLM responses are inherently non-deterministic. Two approaches:

1. **Cache LLM responses**: Hash the prompt and cache the response.
   Same prompt + same cache = deterministic. Cache is per-run.
2. **Separate exploration from verification**: Use LLMs for exploration
   (finding interesting seeds), then verify findings with deterministic
   replay using exact judges.

The recommended approach is (2): LLMs guide *which seeds to explore*,
but the rollouts themselves remain deterministic and reproducible.

## Judge Scoring

```rust
struct JudgeScore {
    judge_name: String,
    is_exact: bool,
    score: Score,
    findings: Vec<Finding>,
}

enum Score {
    PassFail(bool),      // exact judges
    Numeric(f64),        // nuanced judges (0.0 = worst, 1.0 = best)
}

struct Finding {
    severity: Severity,
    description: String,
    relevant_events: Vec<usize>,  // indices into trace
}
```

## Key Design Questions

- How to summarize traces for LLM consumption (full trace is too large)
- Whether to use structured output (JSON mode) or free-form LLM responses
- How to evaluate LLM judge quality (calibration, false positive rate)
- Whether to fine-tune models on known bug patterns
- How to handle LLM latency (async, batched, or between rollouts)
- For code generation: how to give the LLM enough context about
  Convex's UDF API without exceeding context limits
- For code generation: how to handle generated code that fails to
  deploy (syntax errors, type errors, invalid API usage)
- For code generation: how to define invariants for generated code
  (the LLM proposes invariants, but can we trust them?)
- Whether generated UDFs should be one-shot or iteratively refined
  across multiple generation cycles

## Dependencies

- Phase 1 DST framework (Scenario, Runner, BatchRunner)
- Structured event reporting (for trace input to LLMs)
