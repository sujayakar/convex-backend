# Technical Debt Review Report

Date: 2026-02-12  
Branch reviewed: `cursor/technical-debt-review-and-report-77c5` (tracking `main`)

## Scope and method

This review focused on debt that looks like "implemented enough to ship, but not fully completed."

I reviewed:
- Git history patterns (feature churn, fixes, reverts, migration waves).
- Current TODO/HACK/FIXME markers in backend runtime/model and CLI codepaths.
- Blame history on active debt markers to separate old persistent debt from new in-flight work.

## Executive summary

Components are deeply integrated and clearly production-used, but key areas are still in transitional mode.  
The largest cleanup opportunity is finishing the components namespace/data-model migration and removing compatibility shims.

Top risk themes:
1. **Namespace migration remains hybrid** (`by_component_TODO` still in critical runtime paths).
2. **Orphaned component namespaces are tolerated instead of prevented/reconciled.**
3. **Component attribution gaps remain** in HTTP action logging, scheduler virtual tables, and some export paths.
4. **Deploy/push contracts still carry transitional payloads** (`exports`) and CLI has explicit TODO shortcuts.
5. **Known deterministic behavior debt remains in component tests** (`Date.now` and random seeding semantics).

History/churn indicators:
- `git rev-list --count HEAD` => `5146` total commits.
- `git log --grep='component' --regexp-ignore-case` => `378` commits mentioning component(s).
- `git log --grep='revert' --regexp-ignore-case` => `119` revert commits total; `16` also mention components.
- `git log --grep='components push path' --regexp-ignore-case` => `7` commits in the push-path hardening sequence.

---

## Findings and cleanup plan

### 1) Namespace migration still incomplete (`TableNamespace::by_component_TODO`)

**Why this matters**  
The system still relies on a transitional namespace shim that currently resolves to `Global`, which can mask incorrect component scoping and keep migration debt alive across feature areas.

**Current evidence**
- `crates/value/src/table_mapping.rs`: `by_component_TODO()` still exists as explicit transition marker.
- Active call sites remain in:
  - `crates/model/src/components/type_checking.rs`
  - `crates/isolate/src/isolate2/runner.rs`
  - `crates/application/src/application_function_runner/mod.rs`
  - `crates/application/src/lib.rs` (multiple call sites)
- `crates/model/src/components/type_checking.rs` also labels this as a hack:
  - `TODO(CX-6540): Remove hack where we pass in empty mappings.`

**Age signal (blame)**
- `by_component_TODO` introduced in Jun 2024 and still active.
- Type-checking hack around this path dates to Aug 2024.
- Some call sites are newer (e.g. Feb 2025), indicating continued reliance.

**History signal**
- `0126dc9cd`: "Get rid of TableNamespace::TODO()".
- Components push-path migration/hardening continued after this, but the shim remains in use.

**Cleanup plan**
1. Replace each `by_component_TODO` call site with explicit namespace intent:
   - `root_component()` where root-only behavior is intended.
   - `TableNamespace::from(component_id)` where component-scoped behavior is required.
2. Add a lint/check (CI grep gate) that fails new `by_component_TODO` additions.
3. Remove `by_component_TODO` only after all call sites are eliminated and migration tests pass.

**Definition of done**
- 0 runtime call sites of `by_component_TODO`.
- Symbol removed from `table_mapping.rs`.
- Component namespace behavior covered in unit + integration tests for:
  - scheduler cancellation,
  - airbyte record processing,
  - isolate2 table mapping reads.

---

### 2) Orphaned component namespaces are treated as expected steady-state

**Why this matters**  
Code currently assumes incomplete component pushes can leave orphaned tables/namespaces and then works around them in billing/storage metrics and counts. This is safe-ish, but it normalizes inconsistent state and can hide data-model issues.

**Current evidence**
- `crates/database/src/database.rs` explicitly documents:
  - "user table left over from incomplete components push"
  - "orphaned TableNamespace without a component"
  - exclusion of orphaned tables from document counts.

**Age signal (blame)**
- These guardrails were added in mid/late 2025 and early 2026, so this remains active operational debt.

**History signal**
- Prior cleanup waves exist (`clean up orphaned table namespaces`, `delete table docs for deleted tablets`), but orphan handling still exists in hot paths.

**Cleanup plan**
1. Add a first-class orphan reconciliation worker:
   - detect orphaned namespace/table rows,
   - classify as empty vs non-empty,
   - auto-clean empty; escalate non-empty.
2. Emit dedicated metrics:
   - orphan namespace count,
   - orphan non-empty table count,
   - time-to-reconciliation.
3. Add deploy/push failure handling that proactively reconciles partial component state.

**Definition of done**
- No "incomplete components push" assumptions in steady-state read paths.
- Orphan counts are near-zero and alertable.
- Reconciliation path tested in failure-injection push scenarios.

---

### 3) HTTP action/component attribution is still incomplete in logs

**Why this matters**  
HTTP actions are still logged under root component paths in some function-log paths, which weakens observability and attribution for component-heavy deployments.

**Current evidence**
- `crates/application/src/function_log.rs`:
  - `TODO(ENG-7612): Support HTTP actions in components.` appears in multiple paths.
  - Fallback behavior sets HTTP action component path to root.
- Additional TODO remains in table metrics naming:
  - "Thread component path through here."

**Age signal (blame)**
- TODOs persisted since Oct 2024 / Dec 2024.

**History signal**
- Components logging got multiple updates (component path plumbing exists for other UDF types), but HTTP action parity remains unfinished.

**Cleanup plan**
1. Carry component context through HTTP action route/outcome types.
2. Remove root fallback in `FunctionEventSource` for HTTP actions.
3. Add regression tests validating component path in log events for:
   - success,
   - user error,
   - system error,
   - progress events.

**Definition of done**
- No HTTP-action root-path fallback for component actions.
- Logstream and app metrics show correct component attribution.

---

### 4) Scheduled jobs are partially component-aware

**Why this matters**  
Scheduled jobs have component-aware internals, but virtual table exposure and some cancellation paths still use TODO-era assumptions.

**Current evidence**
- `crates/model/src/scheduled_jobs/virtual_table.rs`:
  - `TODO(ENG-6920) include component (job.path.component) in virtual table.`
- `crates/application/src/application_function_runner/mod.rs`:
  - cancellation path still uses `TableNamespace::by_component_TODO()`.

**Age signal (blame)**
- Both markers date to mid-2024 and are still active.

**History signal**
- Scheduled jobs had multiple migration commits, suggesting partial migration completion.

**Cleanup plan**
1. Include component path/id in virtual scheduled-jobs docs.
2. Make cancel/list operations component-namespace explicit.
3. Add cross-component schedule/cancel tests, including malformed virtual IDs.

**Definition of done**
- Scheduled jobs virtual table includes component attribution.
- Scheduler APIs no longer depend on namespace TODO shim.

---

### 5) Streaming export parity is incomplete across endpoints

**Why this matters**  
`document_deltas` still intentionally skips non-root components, while the broader streaming export stack has component-related upgrades. This creates inconsistent behavior between export surfaces.

**Current evidence**
- `crates/database/src/database.rs`:
  - `TODO(ENG-6383): Reenable streaming export for non-root components.`
  - explicit `if !component_id.is_root() { continue; }`.

**Age signal (blame)**
- This guard has persisted since Oct 2024.

**History signal**
- History shows both "Skip non-root components in streaming export" and later enhancements for streaming export selection, indicating a staged migration that never fully converged.

**Cleanup plan**
1. Decide target behavior per endpoint (`document_deltas` vs `list_snapshot`) and document it.
2. If parity is intended, enable non-root export with:
   - feature flag,
   - connector compatibility tests,
   - staged rollout metrics.
3. Remove TODO once rollout is complete.

**Definition of done**
- Endpoint behavior is consistent and documented.
- No root-only skip for components where parity is expected.

---

### 6) CLI components push path still contains explicit ship-time shortcuts

**Why this matters**  
The CLI components flow has known TODO shortcuts around debug bundle output, per-component runtime versioning, and unsupported component node actions.

**Current evidence**
- `npm-packages/convex/src/cli/lib/components.ts`:
  - `TODO(ENG-6972): Actually write the bundles for components.`
  - TODO on using per-component `convex` package versions.
- `npm-packages/convex/src/cli/lib/components/definition/bundle.ts`:
  - `TODO(ENG-7116)` to remove hard error and support node actions in components.

**Age signal (blame)**
- TODOs date from Jul-Sep 2024 and remain.

**History signal**
- Push path changed from optional to default, with repeated stabilization commits (`fix staged indexes`, `test staged backfill`, `remove non-components path`).
- Temporary disable/restore sequences exist for related deploy safety checks, indicating operational pressure during rollout.

**Cleanup plan**
1. Implement debug bundle output for all component bundles (not root-only path).
2. Resolve per-component package-version contract.
3. Implement gated support for component node actions (or codify as permanent non-goal with hard docs + schema validation).

**Definition of done**
- No TODO markers in components push orchestration path.
- CLI behavior matches backend capability matrix and docs.

---

### 7) Start/finish push response contract still carries transitional fields

**Why this matters**  
`start_push`/`finish_push` still include compatibility logic around `exports`, increasing payload complexity and coupling old/new clients.

**Current evidence**
- `crates/application/src/deploy_config.rs`:
  - `TODO(ENG-7533)` in start push response shaping.
  - `TODO(ENG-7533)` in finish push cleanup path.

**Age signal (blame)**
- One TODO dates to Oct 2024; another to Nov 2025, so this remains open and touched.

**History signal**
- `Don't store exports in system metadata` landed, but transport-level compatibility shims remain.

**Cleanup plan**
1. Introduce explicit response versioning for deploy2 start/finish push.
2. Remove `exports` from default response contract once minimum client version threshold is met.
3. Delete compatibility code and dead fields.

**Definition of done**
- Single canonical start/finish push payload format.
- No export-field shims in deploy_config paths.

---

### 8) Determinism debt is acknowledged in component tests but unresolved

**Why this matters**  
Tests explicitly note incorrect temporal/random guarantees across parent/child component execution.

**Current evidence**
- `crates/application/src/tests/components.rs`:
  - TODO that Date.now ordering guarantee is currently opposite of intended.
  - TODO that child random seed should derive from parent for deterministic queries.

**Age signal (blame)**
- Both TODOs date to Sep 2024.

**Cleanup plan**
1. Define deterministic execution contract for nested component calls.
2. Implement seed/timestamp propagation model that preserves cache/replay correctness.
3. Update tests to assert intended contract, not temporary behavior.

**Definition of done**
- Determinism contract is documented and enforced in tests.

---

### 9) Component definition deletion still leaves module system tables

**Why this matters**  
Deleting definitions without module-system-table cleanup risks long-tail metadata bloat and cleanup complexity.

**Current evidence**
- `crates/model/src/components/config.rs`:
  - `TODO: Delete the module system tables.` in definition deletion flow.

**Age signal (blame)**
- Marker has existed since May 2024.

**Cleanup plan**
1. Implement explicit module-system-table cleanup in definition deletion flow.
2. Add safety checks for unmounted/active references before delete.
3. Add integration tests for repeated create/delete cycles.

**Definition of done**
- No stale module system tables after definition deletion.

---

## Recommended cleanup sequencing (90-day plan)

### Phase 0 (Week 1-2): guardrails + observability
- Add CI gate blocking new `by_component_TODO` usage.
- Add orphan namespace metrics and dashboard alerts.
- Create debt burn-down board keyed to ENG/CX tickets above.

### Phase 1 (Week 3-6): data-model migration completion
- Eliminate all `by_component_TODO` call sites.
- Land scheduler + airbyte + isolate2 namespace correctness tests.
- Implement module-system-table deletion cleanup.

### Phase 2 (Week 7-9): feature parity and attribution
- Finish HTTP action component attribution in logs/metrics.
- Finish scheduled jobs virtual table component fields.
- Resolve streaming export endpoint parity decisions and rollout.

### Phase 3 (Week 10-12): contract and CLI simplification
- Remove deploy2 `exports` compatibility shim through versioned API.
- Close CLI TODOs around component bundles and versioning.
- Decide and implement (or formally reject) component node action support.

---

## Immediate "quick wins" (can start this week)

1. Replace easy `by_component_TODO` call sites where the correct component/root namespace is already available in scope.
2. Add a one-shot admin endpoint or maintenance job to report current orphaned namespaces/tables.
3. Add missing component attribution to scheduled-jobs virtual table output (read-only schema extension).
4. Open/refresh linked tracking tickets with explicit "definition of done" from this document.

---

## Suggested owners

- **Runtime/DB team:** namespace migration, orphan reconciliation, streaming export parity.
- **Application/API team:** deploy2 payload versioning, push contract cleanup.
- **CLI/devex team:** components push-path TODOs, node-action support strategy.
- **Observability team:** HTTP action component attribution and metric dimensions.

