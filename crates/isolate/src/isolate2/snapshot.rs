//! Per-entrypoint V8 snapshotting.
//!
//! Creates a V8 snapshot blob that contains compiled+evaluated modules for a
//! given entrypoint's import graph. Restoring a fresh context from this snapshot
//! skips module compilation and evaluation entirely (~130us vs ~2-10ms).
//!
//! The snapshot uses Deno's two-part format:
//! - V8 heap blob (compiled code, evaluated state)
//! - Sidecar data (module map metadata, serialized with serde_json)
//!
//! Combined format: `[V8 blob bytes][sidecar JSON bytes][v8_blob_length as u64 LE]`

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::BTreeMap,
};

use anyhow::Context as _;
use deno_core::{
    v8::{
        self,
        scope,
    },
    ModuleSpecifier,
};
use serde::{
    Deserialize,
    Serialize,
};

use super::callback_context::CallbackContext;
use crate::helpers;

/// V8 snapshot blob with compiled+evaluated modules for one entrypoint.
pub struct EntrypointSnapshot {
    /// Combined V8 blob + sidecar data.
    data: Vec<u8>,
}

/// Module map metadata serialized alongside the V8 blob.
#[derive(Serialize, Deserialize)]
pub(crate) struct SnapshotSidecar {
    /// Modules in the order they were stored via `add_context_data`.
    /// Each entry's index corresponds to the V8 context data index used
    /// to retrieve the `v8::Module` handle on restore.
    modules: Vec<SnapshotModuleEntry>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SnapshotModuleEntry {
    pub url: String,
    pub source_map: Option<String>,
}

const LZ4_MAGIC: &[u8; 4] = b"LZ4S";

impl EntrypointSnapshot {
    /// Raw snapshot bytes (may be LZ4-compressed).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn from_data(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// LZ4-compress in place.
    pub fn compress(&mut self) {
        if self.data.starts_with(LZ4_MAGIC) {
            return;
        }
        let compressed = lz4_flex::compress_prepend_size(&self.data);
        let mut result = Vec::with_capacity(LZ4_MAGIC.len() + compressed.len());
        result.extend_from_slice(LZ4_MAGIC);
        result.extend_from_slice(&compressed);
        self.data = result;
    }

    fn decompressed_data(&self) -> Cow<'_, [u8]> {
        if self.data.starts_with(LZ4_MAGIC) {
            let compressed = &self.data[LZ4_MAGIC.len()..];
            let decompressed = lz4_flex::decompress_size_prepended(compressed)
                .expect("LZ4 decompression failed");
            Cow::Owned(decompressed)
        } else {
            Cow::Borrowed(&self.data)
        }
    }

    /// Split into V8 blob and sidecar.
    pub(crate) fn split(&self) -> (Vec<u8>, SnapshotSidecar) {
        let raw = self.decompressed_data();
        let len_bytes: [u8; 8] = raw[raw.len() - 8..]
            .try_into()
            .expect("snapshot too small");
        let v8_len = u64::from_le_bytes(len_bytes) as usize;
        let v8_blob = raw[..v8_len].to_vec();
        let sidecar_bytes = &raw[v8_len..raw.len() - 8];
        let sidecar: SnapshotSidecar =
            serde_json::from_slice(sidecar_bytes).expect("corrupt snapshot sidecar");
        (v8_blob, sidecar)
    }

    fn combine(v8_blob: v8::StartupData, sidecar: &SnapshotSidecar) -> Self {
        let v8_len = v8_blob.len() as u64;
        let sidecar_bytes = serde_json::to_vec(sidecar).expect("failed to serialize sidecar");
        let mut data = Vec::with_capacity(v8_blob.len() + sidecar_bytes.len() + 8);
        data.extend_from_slice(&v8_blob);
        data.extend_from_slice(&sidecar_bytes);
        data.extend_from_slice(&v8_len.to_le_bytes());
        Self { data }
    }
}

thread_local! {
    static SNAPSHOT_MODULE_MAP: RefCell<BTreeMap<String, v8::Global<v8::Module>>> =
        RefCell::new(BTreeMap::new());
}

fn snapshot_resolve_module<'a>(
    _context: v8::Local<'a, v8::Context>,
    specifier: v8::Local<'a, v8::String>,
    _import_assertions: v8::Local<'a, v8::FixedArray>,
    referrer: v8::Local<'a, v8::Module>,
) -> Option<v8::Local<'a, v8::Module>> {
    use deno_core::v8::callback_scope;
    callback_scope!(unsafe let scope, _context);

    let specifier_str = specifier.to_rust_string_lossy(&scope);

    let resolved = if specifier_str.starts_with("convex:") {
        specifier_str
    } else {
        // Relative import -- resolve against referrer URL.
        let referrer_global = v8::Global::new(&scope, referrer);
        let referrer_url = SNAPSHOT_MODULE_MAP.with(|map| {
            let map = map.borrow();
            for (url, module) in map.iter() {
                if module == &referrer_global {
                    return Some(url.clone());
                }
            }
            None
        })?;
        let referrer_spec = ModuleSpecifier::parse(&referrer_url).ok()?;
        deno_core::resolve_import(&specifier_str, referrer_spec.as_str())
            .ok()?
            .to_string()
    };

    SNAPSHOT_MODULE_MAP.with(|map| {
        let map = map.borrow();
        let module_global = map.get(&resolved)?;
        Some(v8::Local::new(&scope, module_global))
    })
}

/// External references needed for snapshots that include Convex callbacks.
fn snapshot_external_references() -> Vec<v8::ExternalReference> {
    crate::udf_runtime::external_references()
}

/// A module to compile into the snapshot.
pub struct SnapshotModule<'a> {
    pub url: &'a str,
    pub source: &'a str,
    pub source_map: Option<&'a str>,
}

/// Create a per-entrypoint snapshot blob.
///
/// Takes the base snapshot (with setup.js runtime), compiles and evaluates the
/// given modules into a new context, and produces a snapshot blob. Contexts
/// restored from this blob will have all modules pre-compiled and evaluated.
///
/// `modules` must be in topological order (dependencies before dependents).
/// `entrypoint` is the module to evaluate (which triggers evaluation of its
/// import graph).
pub fn create_entrypoint_snapshot(
    modules: &[SnapshotModule<'_>],
    entrypoint: &SnapshotModule<'_>,
) -> anyhow::Result<EntrypointSnapshot> {
    let base_snapshot = crate::udf_runtime::base_snapshot();
    let external_refs = snapshot_external_references();

    let mut isolate = v8::Isolate::snapshot_creator_from_existing_snapshot(
        Cow::Borrowed(base_snapshot).into(),
        Some(Cow::Owned(external_refs)),
        None,
    );

    let mut sidecar = SnapshotSidecar {
        modules: Vec::new(),
    };

    {
        scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let context_scope = &mut v8::ContextScope::new(scope, context);

        wire_convex_callbacks(context_scope, context)?;

        SNAPSHOT_MODULE_MAP.with(|map| map.borrow_mut().clear());

        let all_modules = modules.iter().chain(std::iter::once(entrypoint));
        for module in all_modules {
            let url = format!("convex:/{}", module.url);
            let name_str = v8::String::new(context_scope, &url)
                .context("Failed to create module name string")?;
            let source_str = v8::String::new(context_scope, module.source)
                .context("Failed to create module source string")?;
            let origin = helpers::module_origin(context_scope, name_str);
            let mut v8_source = v8::script_compiler::Source::new(source_str, Some(&origin));
            let compiled = v8::script_compiler::compile_module(context_scope, &mut v8_source)
                .ok_or_else(|| anyhow::anyhow!("Failed to compile module {}", module.url))?;
            let module_global = v8::Global::new(context_scope, compiled);
            SNAPSHOT_MODULE_MAP.with(|map| {
                map.borrow_mut().insert(url.clone(), module_global);
            });
            sidecar.modules.push(SnapshotModuleEntry {
                url,
                source_map: module.source_map.map(String::from),
            });
        }

        let entrypoint_url = format!("convex:/{}", entrypoint.url);
        let entrypoint_module = SNAPSHOT_MODULE_MAP.with(|map| {
            map.borrow()
                .get(&entrypoint_url)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Entrypoint module not found"))
        })?;
        let entrypoint_local = v8::Local::new(context_scope, &entrypoint_module);
        anyhow::ensure!(
            entrypoint_local.instantiate_module(context_scope, snapshot_resolve_module)
                == Some(true),
            "Failed to instantiate entrypoint module"
        );
        let result = entrypoint_local
            .evaluate(context_scope)
            .context("Module evaluation returned None")?;
        let promise = v8::Local::<v8::Promise>::try_from(result)
            .context("Module evaluation did not return a promise")?;
        anyhow::ensure!(
            promise.state() == v8::PromiseState::Fulfilled,
            "Module evaluation promise failed with state {:?}",
            promise.state()
        );

        let module_urls: Vec<String> = SNAPSHOT_MODULE_MAP
            .with(|map| map.borrow().keys().cloned().collect());
        for url in &module_urls {
            let module_global = SNAPSHOT_MODULE_MAP.with(|map| {
                map.borrow()
                    .get(url)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Module {url} not in snapshot map"))
            })?;
            let module_local = v8::Local::new(context_scope, &module_global);
            context_scope.add_context_data(context, module_local);
        }

        SNAPSHOT_MODULE_MAP.with(|map| map.borrow_mut().clear());

        context_scope.set_default_context(context);
    }

    let v8_blob = isolate
        .create_blob(v8::FunctionCodeHandling::Keep)
        .context("Failed to create V8 snapshot blob")?;

    Ok(EntrypointSnapshot::combine(v8_blob, &sidecar))
}

/// Wire Convex.syscall, Convex.asyncSyscall, Convex.op, Convex.asyncOp onto
/// the global Convex object.
fn wire_convex_callbacks(
    scope: &mut v8::PinScope<'_, '_>,
    context: v8::Local<v8::Context>,
) -> anyhow::Result<()> {
    let global = context.global(scope);
    let convex_key = v8::String::new(scope, "Convex")
        .context("Failed to create Convex key")?;
    let convex_value: v8::Local<v8::Object> = global
        .get(scope, convex_key.into())
        .context("Missing global.Convex")?
        .try_into()
        .context("global.Convex is not an object")?;

    let syscall_fn = v8::FunctionTemplate::new(scope, CallbackContext::syscall)
        .get_function(scope)
        .context("Failed to get syscall function")?;
    let key = v8::String::new(scope, "syscall").context("Failed to create key")?;
    convex_value.set(scope, key.into(), syscall_fn.into());

    let async_syscall_fn = v8::FunctionTemplate::new(scope, CallbackContext::async_syscall)
        .get_function(scope)
        .context("Failed to get async_syscall function")?;
    let key = v8::String::new(scope, "asyncSyscall").context("Failed to create key")?;
    convex_value.set(scope, key.into(), async_syscall_fn.into());

    let op_fn = v8::FunctionTemplate::new(scope, CallbackContext::op)
        .get_function(scope)
        .context("Failed to get op function")?;
    let key = v8::String::new(scope, "op").context("Failed to create key")?;
    convex_value.set(scope, key.into(), op_fn.into());

    let async_op_fn = v8::FunctionTemplate::new(scope, CallbackContext::start_async_op)
        .get_function(scope)
        .context("Failed to get start_async_op function")?;
    let key = v8::String::new(scope, "asyncOp").context("Failed to create key")?;
    convex_value.set(scope, key.into(), async_op_fn.into());

    Ok(())
}

/// Create a fresh V8 isolate from a group snapshot.
pub fn create_isolate_from_group_snapshot(snapshot: &GroupSnapshot) -> v8::OwnedIsolate {
    let (v8_blob, _sidecar) = snapshot.split();
    let external_refs = snapshot_external_references();
    v8::Isolate::new(
        v8::CreateParams::default()
            .snapshot_blob(v8_blob.into())
            .external_references(external_refs.into()),
    )
}

/// Restore the module map for a specific context index from a group snapshot.
pub(crate) fn restore_group_module_map(
    scope: &mut v8::PinScope<'_, '_>,
    snapshot: &GroupSnapshot,
    context_index: usize,
) -> anyhow::Result<super::context_state::ModuleMap> {
    let (_v8_blob, sidecar) = snapshot.split();
    let entry = sidecar.contexts.get(context_index).ok_or_else(|| {
        anyhow::anyhow!("Context index {context_index} out of range")
    })?;
    super::context_state::ModuleMap::from_snapshot(scope, &entry.modules)
}

/// Create a fresh V8 isolate from an entrypoint snapshot.
///
/// The isolate's default context contains all modules pre-compiled and
/// evaluated. Use `restore_module_map` to reconstruct the Rust-side ModuleMap
/// from the V8 context data.
pub fn create_isolate_from_snapshot(snapshot: &EntrypointSnapshot) -> v8::OwnedIsolate {
    let (v8_blob, _sidecar) = snapshot.split();
    let external_refs = snapshot_external_references();
    v8::Isolate::new(
        v8::CreateParams::default()
            .snapshot_blob(v8_blob.into())
            .external_references(external_refs.into()),
    )
}

/// Restore the module map from V8 context data after creating a context from
/// a snapshot. This retrieves the `v8::Module` handles that were stored
/// during snapshot creation.
pub(crate) fn restore_module_map(
    scope: &mut v8::PinScope<'_, '_>,
    snapshot: &EntrypointSnapshot,
) -> anyhow::Result<super::context_state::ModuleMap> {
    let (_v8_blob, sidecar) = snapshot.split();
    super::context_state::ModuleMap::from_snapshot(scope, &sidecar.modules)
}

pub(crate) use self::SnapshotModuleEntry as ModuleSnapshotEntry;

use std::collections::HashSet;

/// A plan for creating a multi-context snapshot that packs multiple entrypoints
/// into a single V8 snapshot blob. Shared deps are compiled once; each
/// entrypoint gets its own context.
#[derive(Debug, Clone)]
pub struct SnapshotGroupPlan {
    /// Modules shared by all entrypoints in this group (the "core").
    /// Listed in topological order.
    pub shared_modules: Vec<String>,
    /// Extra dep modules needed by this group but not in the global core.
    pub extra_deps: Vec<String>,
    /// Entrypoint module URLs in this group.
    pub entrypoints: Vec<String>,
}

/// Analyze an import graph and produce optimal snapshot group plans.
///
/// Given a set of entrypoints and their transitive import closures, this
/// computes a shared core (deps used by >50% of entrypoints), groups
/// entrypoints by their residual deps beyond the core, and produces
/// `SnapshotGroupPlan`s with bounded group sizes.
///
/// `closures` maps each entrypoint URL to its transitive import closure
/// (set of all module URLs it depends on, including itself).
/// `max_group_size` bounds how many entrypoints go in one snapshot.
pub fn plan_snapshot_groups(
    closures: &BTreeMap<String, HashSet<String>>,
    max_group_size: usize,
) -> Vec<SnapshotGroupPlan> {
    if closures.is_empty() {
        return vec![];
    }

    // Step 1: Compute module frequencies across all closures.
    let mut module_freq: BTreeMap<String, usize> = BTreeMap::new();
    for closure in closures.values() {
        for module in closure {
            *module_freq.entry(module.clone()).or_default() += 1;
        }
    }
    let threshold = closures.len() / 2;

    // Step 2: Find the shared core (modules in >50% of closures).
    let core: HashSet<String> = module_freq
        .iter()
        .filter(|&(_, &freq)| freq > threshold)
        .map(|(module, _)| module.clone())
        .collect();

    // Step 3: Compute residual for each entrypoint and group by residual.
    let mut groups: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
    for (entrypoint, closure) in closures {
        let mut residual: Vec<String> = closure
            .difference(&core)
            .filter(|m| *m != entrypoint) // Don't include the entrypoint itself
            .cloned()
            .collect();
        residual.sort();
        groups
            .entry(residual)
            .or_default()
            .push(entrypoint.clone());
    }

    // Step 4: Build plans with bounded group sizes.
    let mut core_sorted: Vec<String> = core.into_iter().collect();
    core_sorted.sort();

    let mut plans = Vec::new();
    for (residual, members) in &groups {
        for chunk in members.chunks(max_group_size) {
            plans.push(SnapshotGroupPlan {
                shared_modules: core_sorted.clone(),
                extra_deps: residual.clone(),
                entrypoints: chunk.to_vec(),
            });
        }
    }
    plans
}

/// Sidecar for a multi-context snapshot. Maps each entrypoint to its
/// context index and module metadata.
#[derive(Serialize, Deserialize)]
pub(crate) struct MultiContextSidecar {
    /// Per-context entries. Index in this vec = context snapshot index for
    /// `Context::from_snapshot(scope, index)`.
    pub contexts: Vec<MultiContextEntry>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct MultiContextEntry {
    /// The entrypoint module URL for this context.
    pub entrypoint_url: String,
    /// Module metadata for restoring the ModuleMap.
    pub modules: Vec<SnapshotModuleEntry>,
}

/// A multi-context snapshot containing multiple entrypoints sharing the same
/// isolate-level heap (compiled deps).
pub struct GroupSnapshot {
    data: Vec<u8>,
}

impl GroupSnapshot {
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn from_data(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// LZ4-compress the snapshot data for storage savings.
    pub fn compress(&mut self) {
        if self.data.starts_with(LZ4_MAGIC) {
            return;
        }
        let compressed = lz4_flex::compress_prepend_size(&self.data);
        let mut result = Vec::with_capacity(LZ4_MAGIC.len() + compressed.len());
        result.extend_from_slice(LZ4_MAGIC);
        result.extend_from_slice(&compressed);
        self.data = result;
    }

    fn decompressed_data(&self) -> Cow<'_, [u8]> {
        if self.data.starts_with(LZ4_MAGIC) {
            let compressed = &self.data[LZ4_MAGIC.len()..];
            let decompressed = lz4_flex::decompress_size_prepended(compressed)
                .expect("LZ4 decompression failed");
            Cow::Owned(decompressed)
        } else {
            Cow::Borrowed(&self.data)
        }
    }

    /// Split into V8 blob and sidecar.
    pub(crate) fn split(&self) -> (Vec<u8>, MultiContextSidecar) {
        let raw = self.decompressed_data();
        let len_bytes: [u8; 8] = raw[raw.len() - 8..]
            .try_into()
            .expect("snapshot too small");
        let v8_len = u64::from_le_bytes(len_bytes) as usize;
        let v8_blob = raw[..v8_len].to_vec();
        let sidecar_bytes = &raw[v8_len..raw.len() - 8];
        let sidecar: MultiContextSidecar =
            serde_json::from_slice(sidecar_bytes).expect("corrupt group snapshot sidecar");
        (v8_blob, sidecar)
    }

    fn combine(v8_blob: v8::StartupData, sidecar: &MultiContextSidecar) -> Self {
        let v8_len = v8_blob.len() as u64;
        let sidecar_bytes = serde_json::to_vec(sidecar).expect("failed to serialize sidecar");
        let mut data = Vec::with_capacity(v8_blob.len() + sidecar_bytes.len() + 8);
        data.extend_from_slice(&v8_blob);
        data.extend_from_slice(&sidecar_bytes);
        data.extend_from_slice(&v8_len.to_le_bytes());
        Self { data }
    }

    /// Find the context index for a given entrypoint URL.
    pub fn context_index_for(&self, entrypoint_url: &str) -> Option<usize> {
        let (_, sidecar) = self.split();
        sidecar
            .contexts
            .iter()
            .position(|c| c.entrypoint_url == entrypoint_url)
    }
}

/// Create a multi-context snapshot from a `SnapshotGroupPlan`.
///
/// All shared + extra dep modules are compiled once in the isolate. Each
/// entrypoint gets its own context via `add_context()`. Shared deps are
/// naturally deduplicated in the isolate-level heap.
///
/// `module_source` maps module URL to (source, optional source_map).
pub fn create_group_snapshot(
    plan: &SnapshotGroupPlan,
    module_source: &BTreeMap<String, (String, Option<String>)>,
) -> anyhow::Result<GroupSnapshot> {
    let base_snapshot = crate::udf_runtime::base_snapshot();
    let external_refs = snapshot_external_references();

    let mut isolate = v8::Isolate::snapshot_creator_from_existing_snapshot(
        Cow::Borrowed(base_snapshot).into(),
        Some(Cow::Owned(external_refs)),
        None,
    );

    let mut sidecar = MultiContextSidecar {
        contexts: Vec::new(),
    };

    {
        scope!(let scope, &mut isolate);

        for (ctx_idx, entrypoint_url) in plan.entrypoints.iter().enumerate() {
            let context = v8::Context::new(scope, v8::ContextOptions::default());

            let mut context_modules = Vec::new();
            {
                let context_scope = &mut v8::ContextScope::new(scope, context);

                wire_convex_callbacks(context_scope, context)?;

                SNAPSHOT_MODULE_MAP.with(|map| map.borrow_mut().clear());

                let all_module_urls: Vec<&String> = plan
                    .shared_modules
                    .iter()
                    .chain(plan.extra_deps.iter())
                    .chain(std::iter::once(entrypoint_url))
                    .collect();

                for module_url in &all_module_urls {
                    let url = if module_url.starts_with("convex:") {
                        module_url.to_string()
                    } else {
                        format!("convex:/{module_url}")
                    };

                    let already_compiled =
                        SNAPSHOT_MODULE_MAP.with(|map| map.borrow().contains_key(&url));
                    if already_compiled {
                        continue;
                    }

                    let (source, source_map) = module_source.get(module_url.as_str()).ok_or_else(
                        || anyhow::anyhow!("Missing source for module {module_url}"),
                    )?;
                    let name_str = v8::String::new(context_scope, &url)
                        .context("Failed to create module name string")?;
                    let source_str = v8::String::new(context_scope, source)
                        .context("Failed to create module source string")?;
                    let origin = helpers::module_origin(context_scope, name_str);
                    let mut v8_source =
                        v8::script_compiler::Source::new(source_str, Some(&origin));
                    let compiled =
                        v8::script_compiler::compile_module(context_scope, &mut v8_source)
                            .ok_or_else(|| {
                                anyhow::anyhow!("Failed to compile module {module_url}")
                            })?;
                    let module_global = v8::Global::new(context_scope, compiled);
                    SNAPSHOT_MODULE_MAP.with(|map| {
                        map.borrow_mut().insert(url.clone(), module_global);
                    });
                    context_modules.push(SnapshotModuleEntry {
                        url: url.clone(),
                        source_map: source_map.clone(),
                    });
                }

                let entry_url = if entrypoint_url.starts_with("convex:") {
                    entrypoint_url.clone()
                } else {
                    format!("convex:/{entrypoint_url}")
                };
                let entrypoint_module = SNAPSHOT_MODULE_MAP.with(|map| {
                    map.borrow()
                        .get(&entry_url)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Entrypoint module not found: {entry_url}"))
                })?;
                let entrypoint_local = v8::Local::new(context_scope, &entrypoint_module);
                anyhow::ensure!(
                    entrypoint_local
                        .instantiate_module(context_scope, snapshot_resolve_module)
                        == Some(true),
                    "Failed to instantiate entrypoint {entrypoint_url}"
                );
                let result = entrypoint_local
                    .evaluate(context_scope)
                    .context("Module evaluation returned None")?;
                let promise = v8::Local::<v8::Promise>::try_from(result)
                    .context("Module evaluation did not return a promise")?;
                anyhow::ensure!(
                    promise.state() == v8::PromiseState::Fulfilled,
                    "Module evaluation promise failed for {entrypoint_url}"
                );

                let module_urls: Vec<String> =
                    SNAPSHOT_MODULE_MAP.with(|map| map.borrow().keys().cloned().collect());
                for url in &module_urls {
                    let module_global = SNAPSHOT_MODULE_MAP.with(|map| {
                        map.borrow()
                            .get(url)
                            .cloned()
                            .ok_or_else(|| anyhow::anyhow!("Module {url} not in snapshot map"))
                    })?;
                    let module_local = v8::Local::new(context_scope, &module_global);
                    context_scope.add_context_data(context, module_local);
                }

                SNAPSHOT_MODULE_MAP.with(|map| map.borrow_mut().clear());
            }

            if ctx_idx == 0 {
                scope.set_default_context(context);
            } else {
                scope.add_context(context);
            }

            sidecar.contexts.push(MultiContextEntry {
                entrypoint_url: entrypoint_url.clone(),
                modules: context_modules,
            });
        }
    }

    let v8_blob = isolate
        .create_blob(v8::FunctionCodeHandling::Keep)
        .context("Failed to create multi-context snapshot blob")?;

    Ok(GroupSnapshot::combine(v8_blob, &sidecar))
}

use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::Mutex;
use sync_types::CanonicalizedModulePath;

/// Cache key: identifies a specific entrypoint's import graph for a deployment.
#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct SnapshotCacheKey {
    /// Deployment/instance identifier (same as isolate1's `client_id`).
    pub client_id: String,
    /// The UDF's entrypoint module path.
    pub entrypoint_module: CanonicalizedModulePath,
}

/// Default maximum number of snapshots to keep in the cache.
const DEFAULT_MAX_SNAPSHOTS: usize = 256;

/// In-memory LRU-like cache of per-entrypoint V8 snapshot blobs.
///
/// Thread-safe, lock-based. Snapshots are immutable once created and shared
/// via `Arc`. The cache is keyed by `(client_id, entrypoint_module)`.
///
/// When the cache exceeds `max_capacity`, the oldest entries are evicted.
#[derive(Clone)]
pub struct SnapshotCache {
    inner: Arc<Mutex<SnapshotCacheInner>>,
}

struct SnapshotCacheInner {
    entries: HashMap<SnapshotCacheKey, Arc<EntrypointSnapshot>>,
    /// Insertion order for simple FIFO eviction.
    insertion_order: Vec<SnapshotCacheKey>,
    max_capacity: usize,
}

impl SnapshotCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_SNAPSHOTS)
    }

    pub fn with_capacity(max_capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SnapshotCacheInner {
                entries: HashMap::new(),
                insertion_order: Vec::new(),
                max_capacity,
            })),
        }
    }

    /// Look up a cached snapshot for the given key.
    pub fn get(&self, key: &SnapshotCacheKey) -> Option<Arc<EntrypointSnapshot>> {
        self.inner.lock().entries.get(key).cloned()
    }

    /// Store a snapshot in the cache, evicting old entries if over capacity.
    pub fn insert(&self, key: SnapshotCacheKey, snapshot: Arc<EntrypointSnapshot>) {
        let mut inner = self.inner.lock();
        if inner.entries.contains_key(&key) {
            inner.entries.insert(key, snapshot);
            return;
        }
        while inner.entries.len() >= inner.max_capacity && !inner.insertion_order.is_empty() {
            let oldest = inner.insertion_order.remove(0);
            inner.entries.remove(&oldest);
        }
        inner.insertion_order.push(key.clone());
        inner.entries.insert(key, snapshot);
    }

    /// Invalidate all snapshots for a given client_id (e.g., on redeploy).
    pub fn invalidate_client(&self, client_id: &str) {
        let mut inner = self.inner.lock();
        inner.entries.retain(|k, _| k.client_id != client_id);
        inner
            .insertion_order
            .retain(|k| k.client_id != client_id);
    }
}
