use std::cell::RefCell;

use common::{
    log_lines::{
        LogLevel,
        SystemLogMetadata,
    },
    runtime::UnixTimestamp,
    types::{
        EnvVarName,
        EnvVarValue,
    },
};
use deno_core::{
    v8::{
        self,
        scope,
    },
    ModuleSpecifier,
};
use isolate::{
    isolate2::{
        context::Context,
        environment::{
            Environment,
            EnvironmentOutcome,
        },
        session::Session,
        snapshot::{
            self,
            EntrypointSnapshot,
            GroupSnapshot,
            SnapshotGroupPlan,
            SnapshotModule,
        },
        thread::Thread,
    },
    ConcurrencyLimiter,
};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use runtime::prod::ProdRuntime;
use serde_json::Value as JsonValue;
use value::NamespacedTableMapping;

fn main() {
    isolate::client::initialize_v8();
    divan::main();
}

// ---------------------------------------------------------------------------
// BenchEnvironment
// ---------------------------------------------------------------------------

struct BenchEnvironment {
    rng: ChaCha12Rng,
}

impl BenchEnvironment {
    fn new() -> Self {
        Self {
            rng: ChaCha12Rng::from_seed([0u8; 32]),
        }
    }
}

impl Environment for BenchEnvironment {
    fn syscall(&mut self, _name: &str, _args: JsonValue) -> anyhow::Result<JsonValue> {
        Ok(JsonValue::Null)
    }
    fn trace(&mut self, _level: LogLevel, _messages: Vec<String>) -> anyhow::Result<()> {
        Ok(())
    }
    fn trace_system(
        &mut self,
        _level: LogLevel,
        _messages: Vec<String>,
        _: SystemLogMetadata,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn rng(&mut self) -> anyhow::Result<&mut ChaCha12Rng> {
        Ok(&mut self.rng)
    }
    fn unix_timestamp(&mut self) -> anyhow::Result<UnixTimestamp> {
        Ok(UnixTimestamp::from_secs_f64(0.0)?)
    }
    fn unix_timestamp_non_deterministic(&mut self) -> anyhow::Result<UnixTimestamp> {
        Ok(UnixTimestamp::from_secs_f64(0.0)?)
    }
    fn get_environment_variable(&mut self, _: EnvVarName) -> anyhow::Result<Option<EnvVarValue>> {
        Ok(None)
    }
    fn start_execution(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn finish_execution(&mut self) -> anyhow::Result<EnvironmentOutcome> {
        Ok(EnvironmentOutcome {
            observed_rng: false,
            observed_time: false,
        })
    }
    fn get_all_table_mappings(&mut self) -> anyhow::Result<NamespacedTableMapping> {
        anyhow::bail!("not supported in bench")
    }
}

// ---------------------------------------------------------------------------
// Real bundled modules (embedded at compile time)
// ---------------------------------------------------------------------------

const DEP_U5LILI2V: &str = include_str!("testdata/_deps/U5LILI2V.js");
const DEP_R3V2WJ2G: &str = include_str!("testdata/_deps/R3V2WJ2G.js");
const DEP_RKEXAGFN: &str = include_str!("testdata/_deps/RKEXAGFN.js");
const DEP_3MCB5ZJS: &str = include_str!("testdata/_deps/3MCB5ZJS.js");
const DEP_4ZOYWIIA: &str = include_str!("testdata/_deps/4ZOYWIIA.js");
const DEP_6BRUGAVX: &str = include_str!("testdata/_deps/6BRUGAVX.js");
const DEP_KYTPMCQL: &str = include_str!("testdata/_deps/KYTPMCQL.js");
const DEP_MF64ESCW: &str = include_str!("testdata/_deps/MF64ESCW.js");
const DEP_F7OEUBKV: &str = include_str!("testdata/_deps/F7OEUBKV.js");
const DEP_3KZI4YFS: &str = include_str!("testdata/_deps/3KZI4YFS.js");
const USER_BASIC: &str = include_str!("testdata/basic.js");
const USER_FETCH: &str = include_str!("testdata/fetch.js");

const LARGE_500KB: &str = include_str!("testdata/large_500kb.js");
const LARGE_1MB: &str = include_str!("testdata/large_1mb.js");
const LARGE_2MB: &str = include_str!("testdata/large_2mb.js");

fn make_url(name: &str) -> ModuleSpecifier {
    ModuleSpecifier::parse(&format!("convex:/{name}")).unwrap()
}

const ALL_DEPS: &[(&str, &str)] = &[
    ("_deps/U5LILI2V.js", DEP_U5LILI2V),
    ("_deps/R3V2WJ2G.js", DEP_R3V2WJ2G),
    ("_deps/RKEXAGFN.js", DEP_RKEXAGFN),
    ("_deps/3MCB5ZJS.js", DEP_3MCB5ZJS),
    ("_deps/4ZOYWIIA.js", DEP_4ZOYWIIA),
    ("_deps/6BRUGAVX.js", DEP_6BRUGAVX),
    ("_deps/KYTPMCQL.js", DEP_KYTPMCQL),
    ("_deps/MF64ESCW.js", DEP_MF64ESCW),
    ("_deps/F7OEUBKV.js", DEP_F7OEUBKV),
    ("_deps/3KZI4YFS.js", DEP_3KZI4YFS),
];

fn load_modules(context: &mut Context, session: &mut Session, user_module: (&str, &str)) {
    context.enter(session, |mut ctx| {
        for (name, source) in ALL_DEPS {
            let url = make_url(name);
            ctx.register_module(&url, source, None).unwrap();
        }
        let url = make_url(user_module.0);
        ctx.register_module(&url, user_module.1, None).unwrap();
        ctx.evaluate_module(&url).unwrap();
    });
}

/// Helper to create an EntrypointSnapshot using the real snapshot API.
fn make_snapshot(
    deps: &[(&str, &str)],
    entrypoint: (&str, &str),
) -> EntrypointSnapshot {
    let dep_modules: Vec<SnapshotModule<'_>> = deps
        .iter()
        .map(|(url, source)| SnapshotModule {
            url,
            source,
            source_map: None,
        })
        .collect();
    let entry = SnapshotModule {
        url: entrypoint.0,
        source: entrypoint.1,
        source_map: None,
    };
    snapshot::create_entrypoint_snapshot(&dep_modules, &entry)
        .expect("Failed to create snapshot")
}

// =============================================================================
// Lower-bound benchmarks (no modules)
// =============================================================================

/// Bare v8::Context::new() on a reused isolate.
#[divan::bench]
fn create_context_bare(bencher: divan::Bencher) {
    let tokio_rt = tokio::runtime::Builder::new_multi_thread().build().unwrap();
    let rt = ProdRuntime::new(&tokio_rt);
    let limiter = ConcurrencyLimiter::unlimited();
    let mut isolate = isolate::isolate::Isolate::new(rt.clone(), None, limiter.clone());
    bencher.bench_local(|| {
        scope!(let scope, isolate.isolate());
        v8::Context::new(scope, v8::ContextOptions::default());
    });
}

/// Full isolate2 Context::new() with callback wiring.
#[divan::bench]
fn create_context_isolate2(bencher: divan::Bencher) {
    let mut thread = Thread::new();
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _context = Context::new(&mut session, env).unwrap();
    });
}

// =============================================================================
// Cold start benchmarks (compile + evaluate every time)
// =============================================================================

#[divan::bench]
fn cold_start_fetch(bencher: divan::Bencher) {
    let mut thread = Thread::new();
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let mut context = Context::new(&mut session, env).unwrap();
        load_modules(&mut context, &mut session, ("fetch.js", USER_FETCH));
    });
}

#[divan::bench]
fn cold_start_500kb(bencher: divan::Bencher) {
    let url = make_url("large_500kb.js");
    let mut thread = Thread::new();
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let mut context = Context::new(&mut session, env).unwrap();
        context.enter(&mut session, |mut ctx| {
            ctx.register_module(&url, LARGE_500KB, None).unwrap();
            ctx.evaluate_module(&url).unwrap();
        });
    });
}

#[divan::bench]
fn cold_start_1mb(bencher: divan::Bencher) {
    let url = make_url("large_1mb.js");
    let mut thread = Thread::new();
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let mut context = Context::new(&mut session, env).unwrap();
        context.enter(&mut session, |mut ctx| {
            ctx.register_module(&url, LARGE_1MB, None).unwrap();
            ctx.evaluate_module(&url).unwrap();
        });
    });
}

#[divan::bench]
fn cold_start_2mb(bencher: divan::Bencher) {
    let url = make_url("large_2mb.js");
    let mut thread = Thread::new();
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let mut context = Context::new(&mut session, env).unwrap();
        context.enter(&mut session, |mut ctx| {
            ctx.register_module(&url, LARGE_2MB, None).unwrap();
            ctx.evaluate_module(&url).unwrap();
        });
    });
}

// =============================================================================
// Snapshot-restored benchmarks (using the real snapshot API)
// =============================================================================

/// Snapshot restore: new isolate + context from snapshot.
#[divan::bench]
fn snapshot_start_fetch(bencher: divan::Bencher) {
    let snapshot = make_snapshot(ALL_DEPS, ("fetch.js", USER_FETCH));
    eprintln!(
        "fetch.js snapshot: {:.1} KB",
        snapshot.data().len() as f64 / 1024.0
    );
    let snapshot = RefCell::new(snapshot);
    bencher.bench_local(|| {
        let snap = snapshot.borrow();
        let mut thread = Thread::new_from_snapshot(&snap);
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _context = Context::new_from_snapshot(&mut session, env, &snap).unwrap();
    });
}

/// Snapshot restore reusing isolate.
#[divan::bench]
fn snapshot_start_fetch_reuse_isolate(bencher: divan::Bencher) {
    let snapshot = make_snapshot(ALL_DEPS, ("fetch.js", USER_FETCH));
    let mut thread = Thread::new_from_snapshot(&snapshot);
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _context = Context::new_from_snapshot(&mut session, env, &snapshot).unwrap();
    });
}

/// Snapshot restore for 500KB module.
#[divan::bench]
fn snapshot_start_500kb(bencher: divan::Bencher) {
    let snapshot = make_snapshot(&[], ("large_500kb.js", LARGE_500KB));
    eprintln!(
        "500KB snapshot: {:.1} KB",
        snapshot.data().len() as f64 / 1024.0
    );
    let mut thread = Thread::new_from_snapshot(&snapshot);
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _context = Context::new_from_snapshot(&mut session, env, &snapshot).unwrap();
    });
}

/// Snapshot restore for 1MB module.
#[divan::bench]
fn snapshot_start_1mb(bencher: divan::Bencher) {
    let snapshot = make_snapshot(&[], ("large_1mb.js", LARGE_1MB));
    eprintln!(
        "1MB snapshot: {:.1} KB",
        snapshot.data().len() as f64 / 1024.0
    );
    let mut thread = Thread::new_from_snapshot(&snapshot);
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _context = Context::new_from_snapshot(&mut session, env, &snapshot).unwrap();
    });
}

/// Snapshot restore for 2MB module.
#[divan::bench]
fn snapshot_start_2mb(bencher: divan::Bencher) {
    let snapshot = make_snapshot(&[], ("large_2mb.js", LARGE_2MB));
    eprintln!(
        "2MB snapshot: {:.1} KB",
        snapshot.data().len() as f64 / 1024.0
    );
    let mut thread = Thread::new_from_snapshot(&snapshot);
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _context = Context::new_from_snapshot(&mut session, env, &snapshot).unwrap();
    });
}

// =============================================================================
// Multi-context group snapshot benchmarks
// =============================================================================

fn make_group_snapshot() -> GroupSnapshot {
    let mut module_source: std::collections::BTreeMap<String, (String, Option<String>)> =
        std::collections::BTreeMap::new();
    for (name, source) in ALL_DEPS {
        module_source.insert(name.to_string(), (source.to_string(), None));
    }
    module_source.insert("basic.js".to_string(), (USER_BASIC.to_string(), None));
    module_source.insert("fetch.js".to_string(), (USER_FETCH.to_string(), None));

    let plan = SnapshotGroupPlan {
        shared_modules: ALL_DEPS.iter().map(|(name, _)| name.to_string()).collect(),
        extra_deps: vec![],
        entrypoints: vec!["basic.js".to_string(), "fetch.js".to_string()],
    };

    snapshot::create_group_snapshot(&plan, &module_source)
        .expect("Failed to create group snapshot")
}

/// Create group snapshot with 2 entrypoints.
#[divan::bench(sample_count = 10)]
fn group_snapshot_create(bencher: divan::Bencher) {
    bencher.bench(|| {
        let snap = make_group_snapshot();
        eprintln!("Group snapshot size: {:.1} KB", snap.data().len() as f64 / 1024.0);
        snap
    });
}

/// Restore default context (index 0) from group snapshot.
#[divan::bench]
fn group_snapshot_restore_ctx0(bencher: divan::Bencher) {
    let snapshot = make_group_snapshot();
    eprintln!(
        "Group snapshot (2 contexts): {:.1} KB",
        snapshot.data().len() as f64 / 1024.0
    );
    let mut thread = Thread::new_from_group_snapshot(&snapshot);
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _ctx = Context::new_from_group_snapshot(&mut session, env, &snapshot, 0).unwrap();
    });
}

/// Restore additional context (index 1) from group snapshot.
#[divan::bench]
fn group_snapshot_restore_ctx1(bencher: divan::Bencher) {
    let snapshot = make_group_snapshot();
    let mut thread = Thread::new_from_group_snapshot(&snapshot);
    bencher.bench_local(|| {
        let mut session = Session::new(&mut thread);
        let env = Box::new(BenchEnvironment::new());
        let _ctx = Context::new_from_group_snapshot(&mut session, env, &snapshot, 1).unwrap();
    });
}

/// Compare 2 separate vs 1 group snapshot.
#[divan::bench(sample_count = 1)]
fn group_vs_separate_comparison(bencher: divan::Bencher) {
    eprintln!("\n=== Group vs Separate Snapshot Comparison ===\n");

    // Separate snapshots.
    let start = std::time::Instant::now();
    let snap_basic = make_snapshot(ALL_DEPS, ("basic.js", USER_BASIC));
    let basic_time = start.elapsed();
    let start = std::time::Instant::now();
    let snap_fetch = make_snapshot(ALL_DEPS, ("fetch.js", USER_FETCH));
    let fetch_time = start.elapsed();
    let separate_total_size = snap_basic.data().len() + snap_fetch.data().len();
    let separate_total_time = basic_time + fetch_time;

    eprintln!("  Separate snapshots:");
    eprintln!("    basic.js: {:.1} KB, {:.1}ms", snap_basic.data().len() as f64 / 1024.0, basic_time.as_secs_f64() * 1000.0);
    eprintln!("    fetch.js: {:.1} KB, {:.1}ms", snap_fetch.data().len() as f64 / 1024.0, fetch_time.as_secs_f64() * 1000.0);
    eprintln!("    Total: {:.1} KB, {:.1}ms", separate_total_size as f64 / 1024.0, separate_total_time.as_secs_f64() * 1000.0);

    // Group snapshot.
    let start = std::time::Instant::now();
    let snap_group = make_group_snapshot();
    let group_time = start.elapsed();

    eprintln!("\n  Group snapshot (2 contexts):");
    eprintln!("    Total: {:.1} KB, {:.1}ms", snap_group.data().len() as f64 / 1024.0, group_time.as_secs_f64() * 1000.0);

    let size_savings = 1.0 - (snap_group.data().len() as f64 / separate_total_size as f64);
    let time_savings = 1.0 - (group_time.as_secs_f64() / separate_total_time.as_secs_f64());
    eprintln!("\n  Savings:");
    eprintln!("    Size: {:.1}% smaller", size_savings * 100.0);
    eprintln!("    Creation time: {:.1}% faster", time_savings * 100.0);
    eprintln!();

    bencher.bench(|| std::hint::black_box(42));
}
