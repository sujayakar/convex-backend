use std::{
    cmp::Ordering,
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use anyhow::Context as AnyhowContext;
use common::{
    bootstrap_model::components::handles::FunctionHandle,
    components::{
        CanonicalizedComponentFunctionPath,
        CanonicalizedComponentModulePath,
        ComponentId,
        ComponentPath,
        PublicFunctionPath,
        Reference,
        ResolvedComponentFunctionPath,
        Resource,
    },
    errors::JsError,
    execution_context::ExecutionContext,
    knobs::{
        ACTION_USER_TIMEOUT,
        ISOLATE_MAX_USER_HEAP_SIZE,
        MAX_REACTOR_CALL_DEPTH,
    },
    log_lines::{
        LogLevel,
        LogLine,
    },
    query::Query,
    query_journal::QueryJournal,
    runtime::{
        Runtime,
        UnixTimestamp,
    },
    sync::spsc,
    types::{
        AllowedVisibility,
        EnvVarName,
        EnvVarValue,
        PersistenceVersion,
        UdfType,
    },
    version::Version,
};
use database::{
    query::TableFilter,
    BootstrapComponentsModel,
    DeveloperQuery,
    Transaction,
};
use errors::{
    ErrorMetadata,
    ErrorMetadataAnyhowExt,
};
use file_storage::TransactionalFileStorage;
use keybroker::FunctionRunnerKeyBroker;
use model::{
    config::module_loader::ModuleLoader,
    environment_variables::EnvironmentVariablesModel,
    file_storage::{
        types::FileStorageEntry,
        BatchKey,
        FileStorageId,
    },
    components::{
        handles::FunctionHandlesModel,
        ComponentsModel,
    },
    modules::user_error::ModuleNotFoundError,
    udf_config::UdfConfigModel,
    virtual_system_mapping,
};
use parking_lot::Mutex;
use rand::{
    RngCore,
    SeedableRng,
};
use sync_types::CanonicalizedUdfPath;
use rand_chacha::ChaCha12Rng;
use serde_json::Value as JsonValue;
use tokio::sync::{
    mpsc::{
        self,
        error::TrySendError,
    },
    oneshot,
    Semaphore,
};
use common::http::fetch::FetchClient;
use keybroker::Identity;
use udf::{
    ActionOutcome,
    validation::{
        validate_schedule_args,
        ValidatedPathAndArgs,
    },
    SyscallTrace,
    UdfOutcome,
};
use crate::{
    ActionCallbacks,
    environment::{
        action::TaskResponseEnum,
        async_op::AsyncOpRequest,
    },
};
use value::{
    identifier::Identifier,
    ConvexArray,
    ConvexObject,
    ConvexValue,
    JsonPackedValue,
    NamespacedTableMapping,
    TableMapping,
    TableName,
    TableNamespace,
    TableNumber,
    TabletIdAndTableNumber,
};

use super::{
    client::{
        AsyncOpCompletion,
        AsyncSyscallCompletion,
        Completions,
        EvaluateResult,
        IsolateThreadClient,
        IsolateThreadRequest,
        PendingAsyncSyscall,
        QueryId,
    },
    context::Context,
    environment::{
        Environment,
        EnvironmentOutcome,
    },
    session::Session,
    thread::Thread,
};
use crate::{
    client::initialize_v8,
    environment::{
        helpers::{
            module_loader::{
                module_specifier_from_path,
                path_from_module_specifier,
            },
            MAX_LOG_LINES,
        },
        udf::{
            async_syscall::{
                AsyncSyscallBatch,
                AsyncSyscallProvider,
                DatabaseSyscallsV1,
                ManagedQuery,
            },
            syscall::{
                syscall_impl,
                SyscallProvider,
            },
            DatabaseUdfEnvironment,
        },
    },
};

fn handle_request(
    session: &mut Session,
    context: &mut Context,
    request: IsolateThreadRequest,
) -> anyhow::Result<()> {
    match request {
        IsolateThreadRequest::RegisterModule {
            name,
            source,
            source_map,
            response,
        } => {
            let result = context.enter(session, |mut ctx| {
                ctx.register_module(&name, &source, source_map)
            });
            response
                .send(result)
                .map_err(|_| anyhow::anyhow!("Canceled"))?;
        },
        IsolateThreadRequest::EvaluateModule { name, response } => {
            let result = context.enter(session, |mut ctx| {
                ctx.evaluate_module(&name)?;
                anyhow::Ok(())
            });
            response
                .send(result)
                .map_err(|_| anyhow::anyhow!("Canceled"))?;
        },
        IsolateThreadRequest::StartFunction {
            udf_type,
            udf_path,
            arguments,
            response,
        } => {
            let r = context.start_function(session, udf_type, udf_path, arguments);
            response.send(r).map_err(|_| anyhow::anyhow!("Canceled"))?;
        },
        IsolateThreadRequest::PollFunction {
            function_id,
            completions,
            response,
        } => {
            let r = context.poll_function(session, function_id, completions);
            response.send(r).map_err(|_| anyhow::anyhow!("Canceled"))?;
        },
        IsolateThreadRequest::StartHttpAction {
            http_module_path,
            routed_path,
            request_json,
            method,
            body,
            response,
        } => {
            let r = context.start_http_action(
                session,
                &http_module_path,
                &routed_path,
                &request_json,
                &method,
                body,
            );
            response.send(r).map_err(|_| anyhow::anyhow!("Canceled"))?;
        },
        IsolateThreadRequest::ExtractStream {
            stream_id,
            response,
        } => {
            let r = context.enter(session, |mut ctx| {
                let state = ctx.context_state_mut()?;
                state.drain_stream(stream_id)
            });
            response.send(r).map_err(|_| anyhow::anyhow!("Canceled"))?;
        },
        IsolateThreadRequest::Shutdown { response } => {
            let r = context.enter(session, |mut ctx| ctx.shutdown());
            response.send(r).map_err(|_| anyhow::anyhow!("Canceled"))?;
        },
    }
    Ok(())
}

async fn v8_thread(
    mut receiver: mpsc::Receiver<IsolateThreadRequest>,
    environment: Box<dyn Environment>,
) -> anyhow::Result<()> {
    let mut thread = Thread::new();
    let mut session = Session::new(&mut thread);
    let mut context = Context::new(&mut session, environment)?;

    while let Some(request) = receiver.recv().await {
        handle_request(&mut session, &mut context, request)?;
    }

    drop(context);
    drop(session);
    drop(thread);

    Ok(())
}

#[derive(Debug, Copy, Clone)]
pub struct SeedData {
    pub rng_seed: [u8; 32],
    pub unix_timestamp: UnixTimestamp,
}

#[derive(Debug)]
enum UdfPhase {
    Importing {
        rng: ChaCha12Rng,
    },
    Executing {
        rng: ChaCha12Rng,
        observed_time: bool,
        observed_rng: bool,
    },
    Finalized,
}

struct UdfEnvironment<RT: Runtime> {
    rt: RT,
    is_system: bool,

    log_line_sender: spsc::Sender<LogLine>,
    lines_logged: usize,

    import_time_seed: SeedData,
    execution_time_seed: SeedData,

    phase: UdfPhase,

    shared: UdfShared<RT>,

    env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,

    component_args: Option<BTreeMap<Identifier, ConvexValue>>,
}

impl<RT: Runtime> UdfEnvironment<RT> {
    pub fn new(
        rt: RT,
        is_system: bool,
        import_time_seed: SeedData,
        execution_time_seed: SeedData,
        shared: UdfShared<RT>,
        env_vars: BTreeMap<EnvVarName, EnvVarValue>,
        system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
        component_args: Option<BTreeMap<Identifier, ConvexValue>>,
        log_line_sender: spsc::Sender<LogLine>,
    ) -> Self {
        let rng = ChaCha12Rng::from_seed(import_time_seed.rng_seed);
        Self {
            rt,
            is_system,

            log_line_sender,
            lines_logged: 0,

            import_time_seed,
            execution_time_seed,

            phase: UdfPhase::Importing { rng },

            shared,
            env_vars,
            system_env_vars,
            component_args,
        }
    }

    fn check_executing(&self) -> anyhow::Result<()> {
        let UdfPhase::Executing { .. } = self.phase else {
            // TODO: Is this right? Should we just be using JsError?
            anyhow::bail!(ErrorMetadata::bad_request(
                "NoDbDuringImport",
                "Can't use database at import time",
            ))
        };
        Ok(())
    }

    fn emit_log_line(&mut self, line: LogLine) -> anyhow::Result<()> {
        anyhow::ensure!(self.lines_logged < MAX_LOG_LINES);
        self.lines_logged += 1;
        if let Err(e) = self.log_line_sender.try_send(line) {
            match e {
                // In this case it's not much use to continue executing JS since the Tokio
                // thread has gone away.
                TrySendError::Closed(..) => anyhow::bail!("Log line receiver disconnected"),
                // If the Tokio thread is processing messages slower than we're streaming them
                // out, fail with a system error to shed load.
                TrySendError::Full(_) => {
                    anyhow::bail!("Log lines produced faster than Tokio thread can consume them")
                },
            }
        }
        Ok(())
    }
}

impl<RT: Runtime> SyscallProvider<RT> for UdfEnvironment<RT> {
    fn table_filter(&self) -> TableFilter {
        if self.is_system {
            TableFilter::IncludePrivateSystemTables
        } else {
            TableFilter::ExcludePrivateSystemTables
        }
    }

    fn lookup_table(&mut self, name: &TableName) -> anyhow::Result<Option<TabletIdAndTableNumber>> {
        self.check_executing()?;
        self.shared.lookup_table(name)
    }

    fn lookup_virtual_table(&mut self, name: &TableName) -> anyhow::Result<Option<TableNumber>> {
        self.check_executing()?;
        self.shared.lookup_virtual_table(name)
    }

    fn component_argument(&self, name: &str) -> anyhow::Result<Option<ConvexValue>> {
        let Some(ref component_args) = self.component_args else {
            return Ok(None);
        };
        let result = match name.parse::<Identifier>() {
            Ok(identifier) => component_args.get(&identifier).cloned(),
            Err(_) => None,
        };
        Ok(result)
    }

    fn start_query(&mut self, query: Query, version: Option<Version>) -> anyhow::Result<QueryId> {
        self.check_executing()?;
        let query_id = self.shared.start_query(query, version);
        Ok(query_id)
    }

    fn cleanup_query(&mut self, query_id: u32) -> bool {
        self.shared.cleanup_query(query_id)
    }
}

impl<RT: Runtime> Environment for UdfEnvironment<RT> {
    fn syscall(&mut self, name: &str, args: JsonValue) -> anyhow::Result<JsonValue> {
        syscall_impl(self, name, args)
    }

    fn trace(
        &mut self,
        level: common::log_lines::LogLevel,
        messages: Vec<String>,
    ) -> anyhow::Result<()> {
        let line = match self.lines_logged.cmp(&(MAX_LOG_LINES - 1)) {
            Ordering::Less => {
                LogLine::new_developer_log_line(
                    level,
                    messages,
                    // Note: accessing the current time here is still deterministic since
                    // we don't externalize the time to the function.
                    self.rt.unix_timestamp(),
                )
            },
            Ordering::Equal => {
                // Add a message about omitting log lines once
                LogLine::new_developer_log_line(
                    LogLevel::Error,
                    vec![format!(
                        "Log overflow (maximum {MAX_LOG_LINES}). Remaining log lines omitted."
                    )],
                    // Note: accessing the current time here is still deterministic since
                    // we don't externalize the time to the function.
                    self.rt.unix_timestamp(),
                )
            },
            Ordering::Greater => {
                return Ok(());
            },
        };
        self.emit_log_line(line)
    }

    fn trace_system(
        &mut self,
        level: common::log_lines::LogLevel,
        messages: Vec<String>,
        system_log_metadata: common::log_lines::SystemLogMetadata,
    ) -> anyhow::Result<()> {
        let line = LogLine::new_system_log_line(
            level,
            messages,
            // Note: accessing the current time here is still deterministic since
            // we don't externalize the time to the function.
            self.rt.unix_timestamp(),
            system_log_metadata,
        );
        self.emit_log_line(line)
    }

    fn rng(&mut self) -> anyhow::Result<&mut rand_chacha::ChaCha12Rng> {
        match self.phase {
            UdfPhase::Importing { ref mut rng } => Ok(rng),
            UdfPhase::Executing {
                ref mut rng,
                ref mut observed_rng,
                ..
            } => {
                *observed_rng = true;
                Ok(rng)
            },
            UdfPhase::Finalized => anyhow::bail!("RNG not available in finalized phase"),
        }
    }

    fn unix_timestamp(&mut self) -> anyhow::Result<UnixTimestamp> {
        let result = match self.phase {
            UdfPhase::Importing { .. } => self.import_time_seed.unix_timestamp,
            UdfPhase::Executing {
                ref mut observed_time,
                ..
            } => {
                *observed_time = true;
                self.execution_time_seed.unix_timestamp
            },
            UdfPhase::Finalized => anyhow::bail!("Time not available in finalized phase"),
        };
        Ok(result)
    }

    fn unix_timestamp_non_deterministic(&mut self) -> anyhow::Result<UnixTimestamp> {
        Ok(self.rt.unix_timestamp())
    }

    fn get_environment_variable(
        &mut self,
        name: common::types::EnvVarName,
    ) -> anyhow::Result<Option<common::types::EnvVarValue>> {
        if let Some(value) = self.env_vars.get(&name) {
            return Ok(Some(value.clone()));
        }
        Ok(self.system_env_vars.get(&name).cloned())
    }

    fn start_execution(&mut self) -> anyhow::Result<()> {
        let UdfPhase::Importing { .. } = self.phase else {
            anyhow::bail!("Phase was already {:?}", self.phase)
        };
        self.phase = UdfPhase::Executing {
            rng: ChaCha12Rng::from_seed(self.execution_time_seed.rng_seed),
            observed_time: false,
            observed_rng: false,
        };
        Ok(())
    }

    fn finish_execution(&mut self) -> anyhow::Result<EnvironmentOutcome> {
        let (observed_time, observed_rng) = match self.phase {
            UdfPhase::Importing { .. } => (false, false),
            UdfPhase::Executing {
                observed_time,
                observed_rng,
                ..
            } => (observed_time, observed_rng),
            UdfPhase::Finalized => {
                anyhow::bail!("Phase was already finalized")
            },
        };
        self.phase = UdfPhase::Finalized;
        self.log_line_sender.close();
        Ok(EnvironmentOutcome {
            observed_rng,
            observed_time,
        })
    }

    fn crypto_rng(&mut self) -> anyhow::Result<crate::environment::crypto_rng::CryptoRng> {
        anyhow::bail!(ErrorMetadata::bad_request(
            "NoCryptoRngInQueriesOrMutations",
            "Can't use cryptographic randomness in queries and mutations. Please consider \
             using an action. See https://docs.convex.dev/functions/actions for more details.",
        ))
    }

    fn get_all_table_mappings(&mut self) -> anyhow::Result<NamespacedTableMapping> {
        self.check_executing()?;
        Ok(self.shared.get_all_table_mappings())
    }
}

/// Handle a `1.0/runUdf` async syscall at the `run_request` level.
///
/// This is handled outside the `AsyncSyscallProvider::run_udf` trait method to
/// avoid a recursive async opaque type issue: `run_isolate_v2_udf` ->
/// `tokio_thread` -> `run_request` -> `provider.run_udf` ->
/// `run_isolate_v2_udf`. By handling it here, we use `Box::pin` and a
/// spawned task to break the cycle.
fn handle_run_udf_syscall<'a, RT: Runtime>(
    rt: &'a RT,
    provider: &'a mut Isolate2SyscallProvider<RT>,
    args: JsonValue,
    module_loader: &'a Arc<dyn ModuleLoader<RT>>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<JsonValue>> + Send + 'a>> {
    Box::pin(handle_run_udf_syscall_inner(rt, provider, args, module_loader))
}

async fn handle_run_udf_syscall_inner<RT: Runtime>(
    rt: &RT,
    provider: &mut Isolate2SyscallProvider<RT>,
    args: JsonValue,
    module_loader: &Arc<dyn ModuleLoader<RT>>,
) -> anyhow::Result<JsonValue> {
    use crate::environment::helpers::{
        with_argument_error,
        ArgName,
    };
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RunUdfArgs {
        udf_type: String,
        name: Option<String>,
        reference: Option<String>,
        function_handle: Option<String>,
        args: JsonValue,
    }

    let RunUdfArgs {
        udf_type: udf_type_str,
        name,
        reference,
        function_handle,
        args,
    } = with_argument_error("runUdf", || Ok(serde_json::from_value(args)?))?;
    let (inner_udf_type, inner_args) = with_argument_error("runUdf", || {
        let udf_type: UdfType = udf_type_str.parse().context(ArgName("udfType"))?;
        let args: ConvexObject = ConvexValue::try_from(args)
            .context(ArgName("args"))?
            .try_into()
            .context(ArgName("args"))?;
        Ok((udf_type, args))
    })?;

    match (provider.udf_type, inner_udf_type) {
        (UdfType::Query, UdfType::Query) => (),
        (UdfType::Mutation, UdfType::Query | UdfType::Mutation) => (),
        _ => {
            anyhow::bail!(ErrorMetadata::bad_request(
                "InvalidFunctionCall",
                format!(
                    "Cannot call a {} function from a {} function",
                    inner_udf_type, provider.udf_type
                )
            ));
        },
    }

    if provider.reactor_depth >= *MAX_REACTOR_CALL_DEPTH {
        anyhow::bail!(ErrorMetadata::bad_request(
            "MaximumCallDepthExceeded",
            "Cross component call depth limit exceeded. Do you have an infinite loop in \
             your app?"
        ));
    }

    let path = match function_handle {
        Some(fh) => {
            let handle: FunctionHandle = with_argument_error("runUdf", || fh.parse())?;
            let path = provider.lookup_function_handle(handle).await?;
            let tx = provider.tx()?;
            let (_, component) = BootstrapComponentsModel::new(tx)
                .must_component_path_to_ids(&path.component)?;
            ResolvedComponentFunctionPath {
                component,
                udf_path: path.udf_path,
                component_path: Some(path.component),
            }
        },
        None => {
            let reference = with_argument_error("runUdf", || match (name, reference) {
                (Some(name), _) => Ok(Reference::Function(name.parse()?)),
                (_, Some(reference)) => Ok(reference.parse()?),
                _ => anyhow::bail!("Missing required argument 'name'"),
            })?;
            let resource = provider.resolve(reference).await?;
            match resource {
                Resource::ResolvedSystemUdf(path) => path,
                Resource::Value(_) => {
                    anyhow::bail!(ErrorMetadata::bad_request(
                        "InvalidResource",
                        "Cannot execute a value resource"
                    ));
                },
                Resource::Function(path) => {
                    let tx = provider.tx()?;
                    let (_, component) = BootstrapComponentsModel::new(tx)
                        .must_component_path_to_ids(&path.component)?;
                    ResolvedComponentFunctionPath {
                        component,
                        udf_path: path.udf_path,
                        component_path: Some(path.component),
                    }
                },
            }
        },
    };

    let called_component_id = path.component;
    let tx = provider.tx()?;
    let path_and_args_result = ValidatedPathAndArgs::new_with_returns_validator(
        AllowedVisibility::All,
        tx,
        PublicFunctionPath::ResolvedComponent(path),
        ConvexArray::try_from(vec![inner_args.into()])?,
        inner_udf_type,
    )
    .await?;
    let (path_and_args, returns_validator) = match path_and_args_result {
        Ok(r) => r,
        Err(e) => {
            anyhow::bail!(ErrorMetadata::bad_request("InvalidArgs", e.message));
        },
    };

    let mut tx = provider.take_tx()?;
    let tokens = tx.begin_subtransaction();

    let query_journal = if provider.is_system && inner_udf_type == UdfType::Query {
        provider.prev_journal.clone()
    } else {
        QueryJournal::new()
    };

    // Execute the sub-UDF via a spawned task with Box::pin to break the
    // recursive async type chain.
    let rng_seed = {
        let mut rng = rt.rng();
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        seed
    };
    let sub_rt = rt.clone();
    let sub_module_loader = module_loader.clone();
    let sub_key_broker = provider.key_broker.clone();
    let sub_system_env_vars = provider.system_env_vars.clone();
    let sub_file_storage = provider.file_storage.clone();
    let sub_context = provider.context.clone();
    let sub_unix_timestamp = provider.unix_timestamp;

    let new_reactor_depth = provider.reactor_depth + 1;
    let (result_tx, result_rx) = oneshot::channel();
    let sub_udf_handle = rt.spawn("run_sub_udf", async move {
        let result = Box::pin(run_isolate_v2_udf_at_depth(
            sub_rt,
            tx,
            sub_module_loader,
            SeedData {
                rng_seed,
                unix_timestamp: sub_unix_timestamp,
            },
            inner_udf_type,
            path_and_args,
            sub_key_broker,
            sub_system_env_vars,
            sub_file_storage,
            sub_context,
            query_journal,
            new_reactor_depth,
        ))
        .await;
        let _ = result_tx.send(result);
    });
    let (mut tx, sub_outcome) = result_rx.await??;
    sub_udf_handle.join().await?;

    match (inner_udf_type, &sub_outcome.result) {
        (UdfType::Mutation, Err(_)) => tx.rollback_subtransaction(tokens)?,
        _ => tx.commit_subtransaction(tokens)?,
    }
    provider.put_tx(tx)?;

    if sub_outcome.observed_identity {
        provider.observe_identity()?;
    }

    if provider.is_system && inner_udf_type == UdfType::Query && sub_outcome.result.is_ok() {
        provider.next_journal = sub_outcome.journal;
    }

    match sub_outcome.result {
        Ok(r) => {
            let result = r.unpack()?;
            let tx = provider.tx()?;
            let table_mapping = tx.table_mapping().namespace(called_component_id.into());
            if let Some(e) =
                returns_validator.check_output(&result, &table_mapping, virtual_system_mapping())
            {
                anyhow::bail!(ErrorMetadata::bad_request("InvalidReturnValue", e.message));
            }
            Ok(ConvexValue::from(result).into())
        },
        Err(e) => {
            anyhow::bail!(e);
        },
    }
}

async fn run_request<RT: Runtime>(
    rt: RT,
    mut tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    execution_time_seed: SeedData,
    client: &mut IsolateThreadClient<RT>,
    udf_type: UdfType,
    path_and_args: ValidatedPathAndArgs,
    shared: UdfShared<RT>,
    mut log_line_receiver: spsc::Receiver<LogLine>,
    key_broker: FunctionRunnerKeyBroker,
    execution_context: ExecutionContext,
    query_journal: QueryJournal,
    file_storage: TransactionalFileStorage<RT>,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    reactor_depth: usize,
) -> anyhow::Result<(Transaction<RT>, UdfOutcome)> {
    let (path, arguments, udf_server_version) = path_and_args.consume();
    let component_id = path.component;
    let udf_path = &path.udf_path;

    // Spawn a separate Tokio thread to receive log lines.
    let (log_line_tx, log_line_rx) = oneshot::channel();
    let log_line_processor = rt.spawn("log_line_processor", async move {
        let mut log_lines: Vec<LogLine> = vec![];
        while let Some(line) = log_line_receiver.recv().await {
            log_lines.push(line);
        }
        let _ = log_line_tx.send(log_lines);
    });

    let r: anyhow::Result<_> = try {
        let mut stack = vec![udf_path.module().clone()];

        while let Some(module_path) = stack.pop() {
            let module_specifier = module_specifier_from_path(&module_path)?;
            let component_module_path = CanonicalizedComponentModulePath {
                component: component_id,
                module_path: module_path.clone(),
            };
            let Some(module_metadata) = module_loader.get_module(&mut tx, component_module_path).await? else {
                let err = ModuleNotFoundError::new(module_path.as_str());
                Err(JsError::from_message(format!("{err}")))?
            };
            let requests = client
                .register_module(
                    module_specifier,
                    module_metadata.source.to_string(),
                    module_metadata.source_map.clone(),
                )
                .await?;
            for requested_module_specifier in requests {
                let module_path = path_from_module_specifier(&requested_module_specifier)?;
                stack.push(module_path);
            }
        }

        let udf_module_specifier = module_specifier_from_path(udf_path.module())?;
        client.evaluate_module(udf_module_specifier.clone()).await?;
        anyhow::Ok(())
    };
    if let Err(e) = r {
        let js_error = e.downcast::<JsError>()?;
        client.shutdown().await?;
        log_line_processor.join().await?;
        let log_lines = log_line_rx.await?.into();
        let outcome = UdfOutcome {
            path: path.for_logging(),
            arguments,
            identity: tx.inert_identity(),
            observed_identity: false,
            rng_seed: execution_time_seed.rng_seed,
            observed_rng: false,
            unix_timestamp: execution_time_seed.unix_timestamp,
            observed_time: false,
            log_lines,
            journal: QueryJournal::new(),
            result: Err(js_error),
            syscall_trace: SyscallTrace::new(),
            udf_server_version,
            memory_in_mb: 0,
            user_execution_time: Some(Duration::ZERO), // Module loading failed before execution
        };
        return Ok((tx, outcome));
    }

    let mut provider = Isolate2SyscallProvider::new(
        tx,
        rt.clone(),
        udf_type,
        component_id,
        udf_path.clone(),
        execution_time_seed.unix_timestamp,
        query_journal,
        udf_path.is_system(),
        shared,
        key_broker,
        execution_context,
        file_storage,
        module_loader.clone(),
        system_env_vars,
    );
    provider.reactor_depth = reactor_depth;
    let r: anyhow::Result<_> = try {
        // Update our shared state with the updated table mappings before reentering
        // user code.
        {
            let tx = provider.tx.as_mut().context("Transaction missing")?;
            provider.shared.update_table_mappings(tx);
        }
        let (function_id, mut result) = client
            .start_function(udf_type, udf_path.clone(), arguments.clone())
            .await?;
        loop {
            let pending = match result {
                EvaluateResult::Ready(r) => break r,
                EvaluateResult::Pending(p) => p,
            };
            let mut completions = Completions::new();

            // TODO: The current implementation returns control to JS after each batch.
            let mut syscall_batch: Option<AsyncSyscallBatch> = None;
            let mut batch_promise_ids = vec![];

            for PendingAsyncSyscall {
                promise_id,
                name,
                args,
            } in pending.async_syscalls
            {
                // Intercept run_udf at the run_request level to avoid the
                // recursive async type issue (run_isolate_v2_udf ->
                // tokio_thread -> run_request -> provider.run_udf ->
                // run_isolate_v2_udf). By handling it here, the recursion
                // goes through Box::pin and a spawned task instead.
                if name == "1.0/runUdf" {
                    // Flush any pending batch first.
                    if let Some(batch) = syscall_batch.take() {
                        let results =
                            DatabaseSyscallsV1::run_async_syscall_batch(&mut provider, batch)
                                .await;
                        assert_eq!(results.len(), batch_promise_ids.len());
                        for (pid, result) in batch_promise_ids.drain(..).zip(results) {
                            completions
                                .async_syscalls
                                .push(AsyncSyscallCompletion { promise_id: pid, result });
                        }
                    }
                    // Handle runUdf directly at the run_request level.
                    let result =
                        handle_run_udf_syscall(&rt, &mut provider, args, &module_loader).await;
                    completions.async_syscalls.push(AsyncSyscallCompletion {
                        promise_id,
                        result: result.map(|v| serde_json::to_string(&v).unwrap()),
                    });
                    continue;
                }

                if let Some(ref mut batch) = syscall_batch
                    && batch.can_push(&name, &args)
                {
                    batch.push(name, args)?;
                    batch_promise_ids.push(promise_id);
                    continue;
                }

                if let Some(batch) = syscall_batch.take() {
                    let results =
                        DatabaseSyscallsV1::run_async_syscall_batch(&mut provider, batch).await;
                    assert_eq!(results.len(), batch_promise_ids.len());

                    for (promise_id, result) in batch_promise_ids.drain(..).zip(results) {
                        completions
                            .async_syscalls
                            .push(AsyncSyscallCompletion { promise_id, result });
                    }
                }

                syscall_batch = Some(AsyncSyscallBatch::new(name, args));
                assert!(batch_promise_ids.is_empty());
                batch_promise_ids.push(promise_id);
            }
            if let Some(batch) = syscall_batch {
                let results =
                    DatabaseSyscallsV1::run_async_syscall_batch(&mut provider, batch).await;
                assert_eq!(results.len(), batch_promise_ids.len());

                for (promise_id, result) in batch_promise_ids.into_iter().zip(results) {
                    completions
                        .async_syscalls
                        .push(AsyncSyscallCompletion { promise_id, result });
                }
            }

            // Async ops don't do anything within UDFs.
            for async_op in pending.async_ops {
                let err = ErrorMetadata::bad_request(
                    format!("No{}InQueriesOrMutations", async_op.request.name_for_error()),
                    format!(
                        "Can't use {} in queries and mutations. Please consider using an action. See https://docs.convex.dev/functions/actions for more details.",
                        async_op.request.description_for_error()
                    ),
                );
                completions.async_ops.push(AsyncOpCompletion {
                    promise_id: async_op.promise_id,
                    result: Err(err.into()),
                });
            }

            // Dynamic imports aren't allowed in UDFs.
            if !pending.dynamic_imports.is_empty() {
                Err(JsError::from_message(
                    "Dynamic imports are not supported in queries and mutations. Please consider \
                     using an action. See https://docs.convex.dev/functions/actions for more \
                     details."
                        .to_string(),
                ))?;
            }

            {
                let tx = provider.tx.as_mut().context("Transaction missing")?;
                provider.shared.update_table_mappings(tx);
            }
            result = client.poll_function(function_id, completions).await?;
        }
    };

    let result = match r {
        Ok(result) => Ok(result),
        Err(e) => {
            let js_error = e.downcast::<JsError>()?;
            Err(js_error)
        },
    };
    let outcome = client.shutdown().await?;
    log_line_processor.join().await?;
    let mut log_lines = log_line_rx.await?;
    {
        let tx = provider.tx()?;
        DatabaseUdfEnvironment::<RT>::add_warnings_to_log_lines(
            &path.clone().for_logging(),
            &arguments,
            client.execution_time()?,
            tx.execution_size(),
            tx.biggest_document_writes(),
            result.as_ref().ok(),
            |warning| {
                log_lines.push(LogLine::new_system_log_line(
                    warning.level,
                    warning.messages,
                    // Note: accessing the current time here is still deterministic since
                    // we don't externalize the time to the function.
                    rt.unix_timestamp(),
                    warning.system_log_metadata,
                ));
            },
        )?;
    }
    let identity = provider.tx()?.inert_identity();
    let observed_identity = provider.observed_identity;
    let next_journal = std::mem::replace(&mut provider.next_journal, QueryJournal::new());
    let syscall_trace = std::mem::replace(&mut provider.syscall_trace, SyscallTrace::new());
    let udf_outcome = UdfOutcome {
        path: path.for_logging(),
        arguments,
        identity,
        observed_identity,
        rng_seed: execution_time_seed.rng_seed,
        observed_rng: outcome.observed_rng,
        unix_timestamp: execution_time_seed.unix_timestamp,
        observed_time: outcome.observed_time,
        log_lines: log_lines.into(),
        journal: next_journal,
        result: result.map(JsonPackedValue::pack),
        syscall_trace,
        udf_server_version,
        memory_in_mb: (*ISOLATE_MAX_USER_HEAP_SIZE / (1 << 20))
            .try_into()
            .unwrap(),
        user_execution_time: Some(client.execution_time()?.elapsed),
    };
    let tx = provider.into_tx()?;
    Ok((tx, udf_outcome))
}

struct UdfShared<RT: Runtime> {
    inner: Arc<Mutex<UdfSharedInner<RT>>>,
}

impl<RT: Runtime> Clone for UdfShared<RT> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<RT: Runtime> UdfShared<RT> {
    pub fn new(table_mapping: TableMapping, component_id: ComponentId) -> Self {
        Self {
            inner: Arc::new(Mutex::new(UdfSharedInner {
                next_query_id: 0,
                queries: BTreeMap::new(),
                table_mapping,
                component_id,
            })),
        }
    }

    fn update_table_mappings(&self, tx: &mut Transaction<RT>) {
        let mut inner = self.inner.lock();
        // TODO: Avoid cloning here if the table mapping hasn't changed.
        inner.table_mapping = tx.table_mapping().clone();
    }

    fn lookup_table(&self, name: &TableName) -> anyhow::Result<Option<TabletIdAndTableNumber>> {
        let inner = self.inner.lock();
        let namespace: TableNamespace = inner.component_id.into();
        Ok(inner.table_mapping.namespace(namespace).id_and_number_if_exists(name))
    }

    fn lookup_virtual_table(&self, name: &TableName) -> anyhow::Result<Option<TableNumber>> {
        let virtual_mapping = virtual_system_mapping();
        let Ok(physical_table_name) = virtual_mapping.virtual_to_system_table(name) else {
            return Ok(None);
        };
        self.lookup_table(physical_table_name)
            .map(|r| r.map(|t| t.table_number))
    }

    fn start_query(&self, query: Query, version: Option<Version>) -> QueryId {
        let mut inner = self.inner.lock();
        let query_id = inner.next_query_id;
        inner.next_query_id += 1;
        inner
            .queries
            .insert(query_id, ManagedQuery::Pending { query, version });
        query_id
    }

    fn take_query(&self, query_id: QueryId) -> Option<ManagedQuery<RT>> {
        let mut inner = self.inner.lock();
        inner.queries.remove(&query_id)
    }

    fn insert_query(&self, query_id: QueryId, query: DeveloperQuery<RT>) {
        let mut inner = self.inner.lock();
        inner.queries.insert(query_id, ManagedQuery::Active(query));
    }

    fn cleanup_query(&self, query_id: u32) -> bool {
        let mut inner = self.inner.lock();
        inner.queries.remove(&query_id).is_some()
    }

    fn get_all_table_mappings(&self) -> NamespacedTableMapping {
        let inner = self.inner.lock();
        let namespace: TableNamespace = inner.component_id.into();
        inner.table_mapping.namespace(namespace)
    }
}

struct UdfSharedInner<RT: Runtime> {
    next_query_id: QueryId,
    queries: BTreeMap<QueryId, ManagedQuery<RT>>,

    table_mapping: TableMapping,
    component_id: ComponentId,
}

struct Isolate2SyscallProvider<RT: Runtime> {
    tx: Option<Transaction<RT>>,
    rt: RT,

    udf_type: UdfType,
    component_id: ComponentId,
    udf_path: CanonicalizedUdfPath,

    shared: UdfShared<RT>,

    unix_timestamp: UnixTimestamp,

    observed_identity: bool,

    prev_journal: QueryJournal,
    next_journal: QueryJournal,

    is_system: bool,

    syscall_trace: SyscallTrace,

    key_broker: FunctionRunnerKeyBroker,
    context: ExecutionContext,

    file_storage: TransactionalFileStorage<RT>,

    #[allow(dead_code)]
    module_loader: Arc<dyn ModuleLoader<RT>>,

    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,

    reactor_depth: usize,
}

impl<RT: Runtime> Isolate2SyscallProvider<RT> {
    fn new(
        tx: Transaction<RT>,
        rt: RT,
        udf_type: UdfType,
        component_id: ComponentId,
        udf_path: CanonicalizedUdfPath,
        unix_timestamp: UnixTimestamp,
        prev_journal: QueryJournal,
        is_system: bool,
        shared: UdfShared<RT>,
        key_broker: FunctionRunnerKeyBroker,
        context: ExecutionContext,
        file_storage: TransactionalFileStorage<RT>,
        module_loader: Arc<dyn ModuleLoader<RT>>,
        system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    ) -> Self {
        Self {
            tx: Some(tx),
            rt,
            udf_type,
            component_id,
            udf_path,
            shared,
            unix_timestamp,
            observed_identity: false,
            prev_journal,
            next_journal: QueryJournal::new(),
            is_system,
            syscall_trace: SyscallTrace::new(),
            key_broker,
            context,
            file_storage,
            module_loader,
            system_env_vars,
            reactor_depth: 0,
        }
    }

    fn tx(&mut self) -> anyhow::Result<&mut Transaction<RT>> {
        self.tx
            .as_mut()
            .context("Transaction missing due to concurrent component call")
    }

    fn take_tx(&mut self) -> anyhow::Result<Transaction<RT>> {
        self.tx
            .take()
            .context("Transaction missing due to concurrent component call")
    }

    fn put_tx(&mut self, tx: Transaction<RT>) -> anyhow::Result<()> {
        anyhow::ensure!(self.tx.is_none(), "Transaction already present");
        self.tx = Some(tx);
        Ok(())
    }

    fn into_tx(mut self) -> anyhow::Result<Transaction<RT>> {
        self.tx
            .take()
            .context("Transaction missing at end of request")
    }
}

impl<RT: Runtime> AsyncSyscallProvider<RT> for Isolate2SyscallProvider<RT> {
    fn rt(&self) -> &RT {
        &self.rt
    }

    fn tx(&mut self) -> anyhow::Result<&mut Transaction<RT>> {
        self.tx()
    }

    fn component(&self) -> anyhow::Result<ComponentId> {
        Ok(self.component_id)
    }

    fn key_broker(&self) -> &FunctionRunnerKeyBroker {
        &self.key_broker
    }

    fn context(&self) -> &ExecutionContext {
        &self.context
    }

    fn observe_identity(&mut self) -> anyhow::Result<()> {
        self.observed_identity = true;
        Ok(())
    }

    fn persistence_version(&self) -> PersistenceVersion {
        self.tx.as_ref().expect("Transaction missing").persistence_version()
    }

    fn is_system(&self) -> bool {
        self.is_system
    }

    fn table_filter(&self) -> TableFilter {
        if self.is_system {
            TableFilter::IncludePrivateSystemTables
        } else {
            TableFilter::ExcludePrivateSystemTables
        }
    }

    fn log_async_syscall(&mut self, name: String, duration: Duration, is_success: bool) {
        self.syscall_trace
            .log_async_syscall(name, duration, is_success);
    }

    fn prev_journal(&mut self) -> &mut QueryJournal {
        &mut self.prev_journal
    }

    fn next_journal(&mut self) -> &mut QueryJournal {
        &mut self.next_journal
    }

    async fn validate_schedule_args(
        &mut self,
        path: CanonicalizedComponentFunctionPath,
        args: Vec<JsonValue>,
        scheduled_ts: UnixTimestamp,
    ) -> anyhow::Result<(CanonicalizedComponentFunctionPath, ConvexArray)> {
        validate_schedule_args(path, args, scheduled_ts, self.unix_timestamp, self.tx()?).await
    }

    async fn file_storage_generate_upload_url(&mut self) -> anyhow::Result<String> {
        let issued_ts = self.unix_timestamp;
        let component = self.component()?;
        let key_broker = self.key_broker.clone();
        let tx = self.tx.as_mut().context("Transaction missing")?;
        self.file_storage
            .generate_upload_url(tx, &key_broker, issued_ts, component)
            .await
    }

    async fn file_storage_get_url_batch(
        &mut self,
        storage_ids: BTreeMap<BatchKey, FileStorageId>,
    ) -> BTreeMap<BatchKey, anyhow::Result<Option<String>>> {
        let component = match self.component() {
            Ok(c) => c,
            Err(e) => {
                return storage_ids
                    .into_keys()
                    .map(|batch_key| (batch_key, Err(e.clone_error())))
                    .collect();
            },
        };
        let tx = match self.tx
            .as_mut()
        {
            Some(tx) => tx,
            None => {
                let e = anyhow::anyhow!("Transaction missing");
                return storage_ids
                    .into_keys()
                    .map(|batch_key| (batch_key, Err(e.clone_error())))
                    .collect();
            },
        };
        self.file_storage
            .get_url_batch(tx, component, storage_ids)
            .await
    }

    async fn file_storage_delete(&mut self, storage_id: FileStorageId) -> anyhow::Result<()> {
        let component = self.component()?;
        let tx = self.tx.as_mut().context("Transaction missing")?;
        self.file_storage
            .delete(tx, component.into(), storage_id)
            .await
    }

    async fn file_storage_get_entry(
        &mut self,
        storage_id: FileStorageId,
    ) -> anyhow::Result<Option<FileStorageEntry>> {
        let component = self.component()?;
        let tx = self.tx.as_mut().context("Transaction missing")?;
        self.file_storage
            .get_file_entry(tx, component.into(), storage_id)
            .await
    }

    fn insert_query(&mut self, query_id: QueryId, query: DeveloperQuery<RT>) {
        self.shared.insert_query(query_id, query)
    }

    fn take_query(&mut self, query_id: QueryId) -> Option<ManagedQuery<RT>> {
        self.shared.take_query(query_id)
    }

    fn cleanup_query(&mut self, query_id: u32) -> bool {
        self.shared.cleanup_query(query_id)
    }

    async fn run_udf(
        &mut self,
        _udf_type: UdfType,
        _path: ResolvedComponentFunctionPath,
        _args: ConvexObject,
    ) -> anyhow::Result<ConvexValue> {
        // TODO: Implement recursive UDF calls (ctx.runQuery/ctx.runMutation).
        // The ownership infrastructure (take_tx/put_tx) is in place. The
        // remaining challenge is breaking the recursive async opaque type chain:
        // run_isolate_v2_udf -> tokio_thread -> run_request -> run_udf ->
        // run_isolate_v2_udf. This requires either boxing the recursive future
        // or restructuring so the recursion happens outside the trait method.
        todo!("run_udf not yet implemented in isolate2");
    }

    async fn create_function_handle(
        &mut self,
        path: CanonicalizedComponentFunctionPath,
    ) -> anyhow::Result<FunctionHandle> {
        FunctionHandlesModel::new(self.tx()?)
            .get_with_component_path(path)
            .await
    }

    async fn resolve(&mut self, reference: Reference) -> anyhow::Result<Resource> {
        let current_component_id = self.component_id;
        let current_udf_path = self.udf_path.clone().into();
        let tx = self.tx()?;
        ComponentsModel::new(tx)
            .resolve(current_component_id, Some(current_udf_path), &reference)
            .await
    }

    async fn lookup_function_handle(
        &mut self,
        handle: FunctionHandle,
    ) -> anyhow::Result<CanonicalizedComponentFunctionPath> {
        FunctionHandlesModel::new(self.tx()?)
            .lookup(handle)
            .await
    }
}

async fn tokio_thread<RT: Runtime>(
    rt: RT,
    tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    execution_time_seed: SeedData,
    mut client: IsolateThreadClient<RT>,
    total_timeout: Duration,
    mut sender: oneshot::Sender<anyhow::Result<(Transaction<RT>, UdfOutcome)>>,
    udf_type: UdfType,
    path_and_args: ValidatedPathAndArgs,
    shared: UdfShared<RT>,
    log_line_receiver: spsc::Receiver<LogLine>,
    key_broker: FunctionRunnerKeyBroker,
    execution_context: ExecutionContext,
    query_journal: QueryJournal,
    file_storage: TransactionalFileStorage<RT>,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    reactor_depth: usize,
) {
    let request = run_request(
        rt.clone(),
        tx,
        module_loader,
        execution_time_seed,
        &mut client,
        udf_type,
        path_and_args,
        shared,
        log_line_receiver,
        key_broker,
        execution_context,
        query_journal,
        file_storage,
        system_env_vars,
        reactor_depth,
    );

    let r = tokio::select! {
        r = request => r,

        // Eventually we'll attempt to cleanup the isolate thread in these conditions.
        _ = rt.wait(total_timeout) => Err(anyhow::anyhow!("Total timeout exceeded")),
        _ = sender.closed() => Err(anyhow::anyhow!("Cancelled")),
    };
    let _ = sender.send(r);
    drop(client);
}

pub async fn run_isolate_v2_udf<RT: Runtime>(
    rt: RT,
    tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    execution_time_seed: SeedData,
    udf_type: UdfType,
    path_and_args: ValidatedPathAndArgs,
    key_broker: FunctionRunnerKeyBroker,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    file_storage: TransactionalFileStorage<RT>,
    context: ExecutionContext,
    query_journal: QueryJournal,
) -> anyhow::Result<(Transaction<RT>, UdfOutcome)> {
    run_isolate_v2_udf_at_depth(
        rt,
        tx,
        module_loader,
        execution_time_seed,
        udf_type,
        path_and_args,
        key_broker,
        system_env_vars,
        file_storage,
        context,
        query_journal,
        0, // top-level call starts at depth 0
    )
    .await
}

async fn run_isolate_v2_udf_at_depth<RT: Runtime>(
    rt: RT,
    mut tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    execution_time_seed: SeedData,
    udf_type: UdfType,
    path_and_args: ValidatedPathAndArgs,
    key_broker: FunctionRunnerKeyBroker,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    file_storage: TransactionalFileStorage<RT>,
    context: ExecutionContext,
    query_journal: QueryJournal,
    reactor_depth: usize,
) -> anyhow::Result<(Transaction<RT>, UdfOutcome)> {
    initialize_v8();

    let semaphore = Arc::new(Semaphore::new(8));
    let user_timeout = Duration::from_secs(5);

    // We actually don't really care about "system timeout" but rather "total
    // timeout", both for how long we're tying up a request thread + serving
    // based on a tx timestamp that may be out of retention.
    // TODO: Decrease this for prod, maybe disable it entirely for tests?
    let total_timeout = Duration::from_secs(128);

    // TODO: Move these into the timeout.
    let udf_config = UdfConfigModel::new(&mut tx, path_and_args.path().component.into())
        .get()
        .await?;
    let import_time_seed = SeedData {
        rng_seed: udf_config
            .as_ref()
            .map(|c| c.import_phase_rng_seed)
            .context("Missing import phase RNG seed")?,
        unix_timestamp: udf_config
            .as_ref()
            .map(|c| c.import_phase_unix_timestamp)
            .context("Missing import phase unix timestamp")?,
    };
    let env_vars = EnvironmentVariablesModel::new(&mut tx).get_all().await?;

    let component_id: ComponentId = path_and_args.path().component.clone().into();
    // Load component arguments for non-root components.
    let component_args = if !component_id.is_root() {
        Some(
            BootstrapComponentsModel::new(&mut tx)
                .load_component_args(component_id)
                .await?,
        )
    } else {
        None
    };

    // TODO: This unconditionally takes a table mapping dep.
    let shared = UdfShared::new(tx.table_mapping().clone(), component_id);
    let (log_line_sender, log_line_receiver) = spsc::channel(32);
    let environment = UdfEnvironment::new(
        rt.clone(),
        path_and_args.path().udf_path.is_system(),
        import_time_seed,
        execution_time_seed,
        shared.clone(),
        env_vars,
        system_env_vars.clone(),
        component_args,
        log_line_sender,
    );

    // The protocol is synchronous, so there should never be more than
    // one pending request at a time.
    let (sender, receiver) = mpsc::channel(1);
    let v8_handle = rt.spawn_thread("isolate2", || async {
        if let Err(e) = v8_thread(receiver, Box::new(environment)).await {
            println!("Error in isolate thread: {e:?}");
        }
    });

    let client = IsolateThreadClient::new(rt.clone(), sender, user_timeout, semaphore);
    let (sender, receiver) = oneshot::channel();
    let tokio_handle = rt.spawn(
        "tokio_thread",
        tokio_thread(
            rt.clone(),
            tx,
            module_loader,
            execution_time_seed,
            client,
            total_timeout,
            sender,
            udf_type,
            path_and_args,
            shared,
            log_line_receiver,
            key_broker,
            context,
            query_journal,
            file_storage,
            system_env_vars,
            reactor_depth,
        ),
    );

    let r = receiver.await??;

    tokio_handle.join().await?;
    v8_handle.join().await?;

    Ok(r)
}

// ---------------------------------------------------------------------------
// Action support
// ---------------------------------------------------------------------------

/// Isolate2 environment for actions. Unlike `UdfEnvironment` (used for
/// queries/mutations), this uses non-deterministic rng and real timestamps.
struct Isolate2ActionEnvironment<RT: Runtime> {
    rt: RT,

    log_line_sender: spsc::Sender<LogLine>,
    lines_logged: usize,

    #[allow(dead_code)]
    import_time_seed: SeedData,
    rng: ChaCha12Rng,
    phase: ActionPhaseState,

    env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
}

#[derive(Debug, PartialEq)]
enum ActionPhaseState {
    Importing,
    Executing,
    Finalized,
}

impl<RT: Runtime> Isolate2ActionEnvironment<RT> {
    fn new(
        rt: RT,
        import_time_seed: SeedData,
        env_vars: BTreeMap<EnvVarName, EnvVarValue>,
        system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
        log_line_sender: spsc::Sender<LogLine>,
    ) -> Self {
        let rng_seed: [u8; 32] = {
            let mut rng = rt.rng();
            let mut seed = [0u8; 32];
            rng.fill_bytes(&mut seed);
            seed
        };
        Self {
            rt,
            log_line_sender,
            lines_logged: 0,
            import_time_seed,
            rng: ChaCha12Rng::from_seed(rng_seed),
            phase: ActionPhaseState::Importing,
            env_vars,
            system_env_vars,
        }
    }

    fn emit_log_line(&mut self, line: LogLine) -> anyhow::Result<()> {
        anyhow::ensure!(self.lines_logged < MAX_LOG_LINES);
        self.lines_logged += 1;
        if let Err(e) = self.log_line_sender.try_send(line) {
            match e {
                TrySendError::Closed(..) => anyhow::bail!("Log line receiver disconnected"),
                TrySendError::Full(_) => {
                    anyhow::bail!("Log lines produced faster than Tokio thread can consume them")
                },
            }
        }
        Ok(())
    }
}

impl<RT: Runtime> Environment for Isolate2ActionEnvironment<RT> {
    fn syscall(&mut self, name: &str, _args: JsonValue) -> anyhow::Result<JsonValue> {
        // Actions don't have synchronous DB syscalls. The only sync syscall
        // that might be called is for table name lookups in error formatting,
        // which we can return an error for.
        anyhow::bail!(ErrorMetadata::bad_request(
            "NoSyscallInActions",
            format!("Syscall {name} is not available in actions"),
        ))
    }

    fn trace(
        &mut self,
        level: LogLevel,
        messages: Vec<String>,
    ) -> anyhow::Result<()> {
        let line = match self.lines_logged.cmp(&(MAX_LOG_LINES - 1)) {
            Ordering::Less => LogLine::new_developer_log_line(
                level,
                messages,
                self.rt.unix_timestamp(),
            ),
            Ordering::Equal => LogLine::new_developer_log_line(
                LogLevel::Error,
                vec![format!(
                    "Log overflow (maximum {MAX_LOG_LINES}). Remaining log lines omitted."
                )],
                self.rt.unix_timestamp(),
            ),
            Ordering::Greater => return Ok(()),
        };
        self.emit_log_line(line)
    }

    fn trace_system(
        &mut self,
        level: LogLevel,
        messages: Vec<String>,
        system_log_metadata: common::log_lines::SystemLogMetadata,
    ) -> anyhow::Result<()> {
        let line = LogLine::new_system_log_line(level, messages, self.rt.unix_timestamp(), system_log_metadata);
        self.emit_log_line(line)
    }

    fn rng(&mut self) -> anyhow::Result<&mut ChaCha12Rng> {
        Ok(&mut self.rng)
    }

    fn unix_timestamp(&mut self) -> anyhow::Result<UnixTimestamp> {
        // Actions use real (non-deterministic) timestamps.
        Ok(self.rt.unix_timestamp())
    }

    fn unix_timestamp_non_deterministic(&mut self) -> anyhow::Result<UnixTimestamp> {
        Ok(self.rt.unix_timestamp())
    }

    fn get_environment_variable(
        &mut self,
        name: EnvVarName,
    ) -> anyhow::Result<Option<EnvVarValue>> {
        if let Some(value) = self.env_vars.get(&name) {
            return Ok(Some(value.clone()));
        }
        Ok(self.system_env_vars.get(&name).cloned())
    }

    fn start_execution(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.phase == ActionPhaseState::Importing,
            "Phase was already {:?}",
            self.phase
        );
        self.phase = ActionPhaseState::Executing;
        // Re-seed RNG for execution phase (non-deterministic).
        let mut rng = self.rt.rng();
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        self.rng = ChaCha12Rng::from_seed(seed);
        Ok(())
    }

    fn finish_execution(&mut self) -> anyhow::Result<EnvironmentOutcome> {
        anyhow::ensure!(
            self.phase != ActionPhaseState::Finalized,
            "Phase was already finalized"
        );
        self.phase = ActionPhaseState::Finalized;
        self.log_line_sender.close();
        // Actions don't track deterministic observation.
        Ok(EnvironmentOutcome {
            observed_rng: false,
            observed_time: false,
        })
    }

    fn crypto_rng(&mut self) -> anyhow::Result<crate::environment::crypto_rng::CryptoRng> {
        Ok(crate::environment::crypto_rng::CryptoRng::new())
    }

    fn get_all_table_mappings(&mut self) -> anyhow::Result<NamespacedTableMapping> {
        anyhow::bail!("get_all_table_mappings unsupported in actions")
    }
}

/// Execute an action using isolate2.
pub async fn run_isolate_v2_action<RT: Runtime>(
    rt: RT,
    mut tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    path_and_args: ValidatedPathAndArgs,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    identity: Identity,
    action_callbacks: Arc<dyn ActionCallbacks>,
    fetch_client: Arc<dyn FetchClient>,
    file_storage: TransactionalFileStorage<RT>,
    log_line_sender: tokio::sync::mpsc::UnboundedSender<LogLine>,
    context: ExecutionContext,
) -> anyhow::Result<ActionOutcome> {
    initialize_v8();

    let semaphore = Arc::new(Semaphore::new(8));
    let user_timeout = *ACTION_USER_TIMEOUT;
    let total_timeout = Duration::from_secs(128);

    let udf_config = UdfConfigModel::new(&mut tx, path_and_args.path().component.into())
        .get()
        .await?;
    let import_time_seed = SeedData {
        rng_seed: udf_config
            .as_ref()
            .map(|c| c.import_phase_rng_seed)
            .context("Missing import phase RNG seed")?,
        unix_timestamp: udf_config
            .as_ref()
            .map(|c| c.import_phase_unix_timestamp)
            .context("Missing import phase unix timestamp")?,
    };
    let env_vars = EnvironmentVariablesModel::new(&mut tx).get_all().await?;

    let component_id: ComponentId = path_and_args.path().component;

    let (action_log_line_sender, action_log_line_receiver) = spsc::channel(32);
    let environment = Isolate2ActionEnvironment::new(
        rt.clone(),
        import_time_seed,
        env_vars,
        system_env_vars,
        action_log_line_sender,
    );

    let (sender, receiver) = mpsc::channel(1);
    let v8_handle = rt.spawn_thread("isolate2_action", || async {
        if let Err(e) = v8_thread(receiver, Box::new(environment)).await {
            println!("Error in isolate2 action thread: {e:?}");
        }
    });

    let client = IsolateThreadClient::new(rt.clone(), sender, user_timeout, semaphore);
    let (result_sender, result_receiver) = oneshot::channel();
    let tokio_handle = rt.spawn(
        "tokio_thread_action",
        tokio_thread_action(
            rt.clone(),
            tx,
            module_loader,
            client,
            total_timeout,
            result_sender,
            path_and_args,
            action_log_line_receiver,
            identity,
            component_id,
            action_callbacks,
            fetch_client,
            file_storage,
            log_line_sender,
            context,
        ),
    );

    let r = result_receiver.await??;

    tokio_handle.join().await?;
    v8_handle.join().await?;

    Ok(r)
}

async fn tokio_thread_action<RT: Runtime>(
    rt: RT,
    mut tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    mut client: IsolateThreadClient<RT>,
    total_timeout: Duration,
    mut sender: oneshot::Sender<anyhow::Result<ActionOutcome>>,
    path_and_args: ValidatedPathAndArgs,
    log_line_receiver: spsc::Receiver<LogLine>,
    identity: Identity,
    component_id: ComponentId,
    action_callbacks: Arc<dyn ActionCallbacks>,
    fetch_client: Arc<dyn FetchClient>,
    file_storage: TransactionalFileStorage<RT>,
    log_line_sender: tokio::sync::mpsc::UnboundedSender<LogLine>,
    context: ExecutionContext,
) {
    let request = run_action_request(
        rt.clone(),
        &mut tx,
        module_loader,
        &mut client,
        path_and_args,
        log_line_receiver,
        identity,
        component_id,
        action_callbacks,
        fetch_client,
        file_storage,
        log_line_sender,
        context,
    );

    let r = tokio::select! {
        r = request => r,
        _ = rt.wait(total_timeout) => Err(anyhow::anyhow!("Total timeout exceeded")),
        _ = sender.closed() => Err(anyhow::anyhow!("Cancelled")),
    };
    let _ = sender.send(r);
    drop(client);
}

async fn run_action_request<RT: Runtime>(
    rt: RT,
    tx: &mut Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    client: &mut IsolateThreadClient<RT>,
    path_and_args: ValidatedPathAndArgs,
    mut log_line_receiver: spsc::Receiver<LogLine>,
    identity: Identity,
    component_id: ComponentId,
    action_callbacks: Arc<dyn ActionCallbacks>,
    fetch_client: Arc<dyn FetchClient>,
    file_storage: TransactionalFileStorage<RT>,
    log_line_sender: tokio::sync::mpsc::UnboundedSender<LogLine>,
    context: ExecutionContext,
) -> anyhow::Result<ActionOutcome> {
    let (path, arguments, udf_server_version) = path_and_args.consume();
    let udf_path = &path.udf_path;

    // Spawn log line forwarder.
    let (log_done_tx, log_done_rx) = oneshot::channel();
    let fwd_log_line_sender = log_line_sender.clone();
    let log_forwarder = rt.spawn("action_log_forwarder", async move {
        let mut log_lines = vec![];
        while let Some(line) = log_line_receiver.recv().await {
            let _ = fwd_log_line_sender.send(line.clone());
            log_lines.push(line);
        }
        let _ = log_done_tx.send(log_lines);
    });

    let r: anyhow::Result<_> = try {
        let mut stack = vec![udf_path.module().clone()];
        while let Some(module_path) = stack.pop() {
            let module_specifier = module_specifier_from_path(&module_path)?;
            let component_module_path = CanonicalizedComponentModulePath {
                component: component_id,
                module_path: module_path.clone(),
            };
            let Some(module_metadata) = module_loader.get_module(tx, component_module_path).await?
            else {
                let err = ModuleNotFoundError::new(module_path.as_str());
                Err(JsError::from_message(format!("{err}")))?
            };
            let requests = client
                .register_module(
                    module_specifier,
                    module_metadata.source.to_string(),
                    module_metadata.source_map.clone(),
                )
                .await?;
            for requested_module_specifier in requests {
                let module_path = path_from_module_specifier(&requested_module_specifier)?;
                stack.push(module_path);
            }
        }
        let udf_module_specifier = module_specifier_from_path(udf_path.module())?;
        client.evaluate_module(udf_module_specifier).await?;
    };
    if let Err(e) = r {
        let js_error = e.downcast::<JsError>()?;
        client.shutdown().await?;
        log_forwarder.join().await?;
        let _log_lines = log_done_rx.await?;
        return Ok(ActionOutcome {
            path: path.for_logging(),
            arguments,
            identity: tx.inert_identity(),
            unix_timestamp: rt.unix_timestamp(),
            result: Err(js_error),
            syscall_trace: SyscallTrace::new(),
            udf_server_version,
            user_execution_time: Some(Duration::ZERO),
        });
    }

    let (function_id, mut result) = client
        .start_function(UdfType::Action, udf_path.clone(), arguments.clone())
        .await?;

    let r: anyhow::Result<_> = try {
        loop {
            let pending = match result {
                EvaluateResult::Ready(r) => break r,
                EvaluateResult::Pending(p) => p,
            };
            let mut completions = Completions::new();

            // Process async syscalls -- route through ActionCallbacks.
            for PendingAsyncSyscall {
                promise_id,
                name,
                args,
            } in pending.async_syscalls
            {
                let syscall_result = handle_action_syscall(
                    &rt,
                    &name,
                    args,
                    &identity,
                    component_id,
                    &action_callbacks,
                    &fetch_client,
                    &context,
                )
                .await;
                // Convert system errors to JsErrors so they become JS
                // promise rejections rather than crashing the action.
                let syscall_result = syscall_result.map_err(|mut e| {
                    if !e.is_deterministic_user_error() {
                        common::errors::report_error_sync(&mut e);
                        anyhow::anyhow!(JsError::from_message(e.user_facing_message()))
                    } else {
                        e
                    }
                });
                completions.async_syscalls.push(AsyncSyscallCompletion {
                    promise_id,
                    result: syscall_result,
                });
            }

            // Process async ops concurrently so timers fire in completion
            // order (shorter delays complete first), matching isolate1's
            // TaskExecutor behavior.
            {
                use futures::{
                    stream::FuturesUnordered,
                    StreamExt,
                };
                let mut op_futures: FuturesUnordered<_> = pending
                    .async_ops
                    .into_iter()
                    .map(|async_op| {
                        let rt = rt.clone();
                        let identity = identity.clone();
                        let action_callbacks = action_callbacks.clone();
                        let fetch_client = fetch_client.clone();
                        let file_storage = file_storage.clone();
                        async move {
                            let op_result = handle_action_async_op(
                                &rt,
                                async_op.request,
                                &identity,
                                component_id,
                                &action_callbacks,
                                &fetch_client,
                                &file_storage,
                            )
                            .await;
                            (async_op.promise_id, op_result)
                        }
                    })
                    .collect();
                while let Some((promise_id, op_result)) = op_futures.next().await {
                    completions.stream_parts.extend(op_result.stream_parts);
                    completions.async_ops.push(AsyncOpCompletion {
                        promise_id,
                        result: op_result.result,
                    });
                }
            }

            // Handle dynamic imports by loading the requested modules.
            for dynamic_import in pending.dynamic_imports {
                let module_path =
                    path_from_module_specifier(&dynamic_import.specifier)?;
                let component_module_path = CanonicalizedComponentModulePath {
                    component: component_id,
                    module_path: module_path.clone(),
                };
                if let Some(module_metadata) =
                    module_loader.get_module(tx, component_module_path).await?
                {
                    let mut stack = vec![];
                    let requests = client
                        .register_module(
                            dynamic_import.specifier.clone(),
                            module_metadata.source.to_string(),
                            module_metadata.source_map.clone(),
                        )
                        .await?;
                    for specifier in requests {
                        stack.push(specifier);
                    }
                    // Load transitive dependencies.
                    while let Some(dep_specifier) = stack.pop() {
                        let dep_path = path_from_module_specifier(&dep_specifier)?;
                        let dep_component_path = CanonicalizedComponentModulePath {
                            component: component_id,
                            module_path: dep_path,
                        };
                        if let Some(dep_metadata) =
                            module_loader.get_module(tx, dep_component_path).await?
                        {
                            let more_requests = client
                                .register_module(
                                    dep_specifier,
                                    dep_metadata.source.to_string(),
                                    dep_metadata.source_map.clone(),
                                )
                                .await?;
                            stack.extend(more_requests);
                        }
                    }
                    client
                        .evaluate_module(dynamic_import.specifier)
                        .await?;
                }
            }

            result = client.poll_function(function_id, completions).await?;
        }
    };

    let result = match r {
        Ok(result) => Ok(result),
        Err(e) => {
            let js_error = e.downcast::<JsError>()?;
            Err(js_error)
        },
    };

    client.shutdown().await?;
    log_forwarder.join().await?;
    let _log_lines = log_done_rx.await?;

    Ok(ActionOutcome {
        path: path.for_logging(),
        arguments,
        identity: tx.inert_identity(),
        unix_timestamp: rt.unix_timestamp(),
        result: result.map(JsonPackedValue::pack),
        syscall_trace: SyscallTrace::new(),
        udf_server_version,
        user_execution_time: Some(client.execution_time()?.elapsed),
    })
}

/// Dispatch an action async syscall to the appropriate ActionCallbacks method.
async fn handle_action_syscall<RT: Runtime>(
    rt: &RT,
    name: &str,
    args: JsonValue,
    identity: &Identity,
    component_id: ComponentId,
    action_callbacks: &Arc<dyn ActionCallbacks>,
    _fetch_client: &Arc<dyn FetchClient>,
    context: &ExecutionContext,
) -> anyhow::Result<String> {
    use crate::environment::helpers::{
        with_argument_error,
        ArgName,
    };
    use crate::helpers::UdfArgsJson;
    use model::components::auth::propagate_component_auth;
    use serde::Deserialize;

    match name {
        "1.0/actions/query" | "1.0/actions/mutation" | "1.0/actions/action" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct RunArgs {
                name: Option<String>,
                reference: Option<String>,
                function_handle: Option<String>,
                args: UdfArgsJson,
            }
            let short_name = name.strip_prefix("1.0/actions/").unwrap();
            let RunArgs {
                name: fn_name,
                reference,
                function_handle,
                args,
            } = with_argument_error(short_name, || Ok(serde_json::from_value(args)?))?;

            let function_path = match function_handle {
                Some(fh) => {
                    let handle: FunctionHandle =
                        with_argument_error(short_name, || fh.parse())?;
                    action_callbacks
                        .lookup_function_handle(identity.clone(), handle)
                        .await?
                },
                None => {
                    let reference = with_argument_error(short_name, || {
                        match (fn_name, reference) {
                            (Some(name), _) => Ok(Reference::Function(name.parse()?)),
                            (_, Some(reference)) => Ok(reference.parse()?),
                            _ => anyhow::bail!("Missing required argument 'name'"),
                        }
                    })?;
                    // For actions, resolve to a function path.
                    match reference {
                        Reference::Function(udf_path) => CanonicalizedComponentFunctionPath {
                            component: ComponentPath::root(),
                            udf_path,
                        },
                        _ => {
                            anyhow::bail!(ErrorMetadata::bad_request(
                                "InvalidReference",
                                "Only function references are supported in actions"
                            ));
                        },
                    }
                },
            };

            let propagated_identity = propagate_component_auth(
                identity,
                component_id,
                function_path.component.is_root(),
            );
            let serialized_args = args.into_serialized_args()?;

            let result = match short_name {
                "query" => {
                    action_callbacks
                        .execute_query(propagated_identity, function_path, serialized_args, context.clone())
                        .await
                        .map_err(crate::environment::helpers::remove_rejected_before_execution)?
                },
                "mutation" => {
                    action_callbacks
                        .execute_mutation(propagated_identity, function_path, serialized_args, context.clone())
                        .await
                        .map_err(crate::environment::helpers::remove_rejected_before_execution)?
                },
                "action" => {
                    action_callbacks
                        .execute_action(propagated_identity, function_path, serialized_args, context.clone())
                        .await
                        .map_err(crate::environment::helpers::remove_rejected_before_execution)?
                },
                _ => unreachable!(),
            };
            let value = result.result?;
            Ok(value.as_str().to_owned())
        },
        "1.0/actions/schedule" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct ScheduleArgs {
                name: Option<String>,
                reference: Option<String>,
                function_handle: Option<String>,
                ts: Option<f64>,
                args: UdfArgsJson,
            }
            let ScheduleArgs {
                name: fn_name,
                reference,
                function_handle,
                ts,
                args,
            } = with_argument_error("schedule", || Ok(serde_json::from_value(args)?))?;

            let function_path = match function_handle {
                Some(fh) => {
                    let handle: FunctionHandle =
                        with_argument_error("schedule", || fh.parse())?;
                    action_callbacks
                        .lookup_function_handle(identity.clone(), handle)
                        .await?
                },
                None => {
                    let reference = with_argument_error("schedule", || {
                        match (fn_name, reference) {
                            (Some(name), _) => Ok(Reference::Function(name.parse()?)),
                            (_, Some(reference)) => Ok(reference.parse()?),
                            _ => anyhow::bail!("Missing required argument 'name'"),
                        }
                    })?;
                    match reference {
                        Reference::Function(udf_path) => CanonicalizedComponentFunctionPath {
                            component: ComponentPath::root(),
                            udf_path,
                        },
                        _ => {
                            anyhow::bail!(ErrorMetadata::bad_request(
                                "InvalidReference",
                                "Only function references are supported for scheduling"
                            ));
                        },
                    }
                },
            };

            let scheduled_ts = match ts {
                Some(ts) => UnixTimestamp::from_secs_f64(ts)?,
                None => rt.unix_timestamp(),
            };
            let serialized_args = args.into_serialized_args()?;
            let job_id = action_callbacks
                .schedule_job(
                    identity.clone(),
                    component_id,
                    function_path,
                    serialized_args,
                    scheduled_ts,
                    context.clone(),
                )
                .await?;
            Ok(serde_json::to_string(&job_id.encode())?)
        },
        "1.0/actions/cancel_job" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct CancelJobArgs {
                id: String,
            }
            let CancelJobArgs { id } =
                with_argument_error("cancel_job", || Ok(serde_json::from_value(args)?))?;
            let virtual_id = with_argument_error("cancel_job", || id.parse().context(ArgName("id")))?;
            action_callbacks
                .cancel_job(identity.clone(), virtual_id)
                .await?;
            Ok("null".to_string())
        },
        "1.0/actions/vectorSearch" => {
            // Vector search returns results that need to be serialized.
            // The callback returns typed results; we need to convert them.
            // For now, pass the args through to the callback which handles
            // the JSON serialization internally.
            anyhow::bail!(ErrorMetadata::bad_request(
                "NotImplemented",
                "vectorSearch not yet implemented in isolate2 actions"
            ))
        },
        "1.0/getUserIdentity" => {
            let user_identity = match identity.clone() {
                Identity::User(identity) => Some(identity.attributes),
                Identity::ActingUser(_, identity) => Some(identity),
                _ => None,
            };
            if let Some(user_identity) = user_identity {
                let json_value: JsonValue = user_identity.try_into()?;
                return Ok(json_value.to_string());
            }
            Ok("null".to_string())
        },
        "1.0/storageGenerateUploadUrl" => {
            // Storage upload URL generation doesn't go through ActionCallbacks
            // in isolate1 either -- it uses the file_storage directly. For now,
            // return an error as this needs the file_storage + key_broker.
            anyhow::bail!(ErrorMetadata::bad_request(
                "NotImplemented",
                "storageGenerateUploadUrl not yet implemented in isolate2 actions"
            ))
        },
        "1.0/storageGetUrl" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct StorageGetUrlArgs {
                storage_id: String,
            }
            let StorageGetUrlArgs { storage_id } =
                with_argument_error("storageGetUrl", || Ok(serde_json::from_value(args)?))?;
            let storage_id = with_argument_error("storageGetUrl", || {
                storage_id.parse().context(ArgName("storageId"))
            })?;
            let url = action_callbacks
                .storage_get_url(identity.clone(), component_id, storage_id)
                .await?;
            Ok(serde_json::to_string(&url)?)
        },
        "1.0/storageDelete" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct StorageDeleteArgs {
                storage_id: String,
            }
            let StorageDeleteArgs { storage_id } =
                with_argument_error("storageDelete", || Ok(serde_json::from_value(args)?))?;
            let storage_id = with_argument_error("storageDelete", || {
                storage_id.parse().context(ArgName("storageId"))
            })?;
            action_callbacks
                .storage_delete(identity.clone(), component_id, storage_id)
                .await?;
            Ok("null".to_string())
        },
        "1.0/storageGetMetadata" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct StorageGetMetadataArgs {
                storage_id: String,
            }
            let StorageGetMetadataArgs { storage_id } =
                with_argument_error("storageGetMetadata", || Ok(serde_json::from_value(args)?))?;
            let storage_id = with_argument_error("storageGetMetadata", || {
                storage_id.parse().context(ArgName("storageId"))
            })?;
            let entry = action_callbacks
                .storage_get_file_entry(identity.clone(), component_id, storage_id)
                .await?;
            match entry {
                Some((_component, file_entry)) => {
                    Ok(serde_json::to_string(&file_entry.storage_id.to_string())?)
                },
                None => Ok("null".to_string()),
            }
        },
        "1.0/createFunctionHandle" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct CreateFunctionHandleArgs {
                name: Option<String>,
                function_handle: Option<String>,
                reference: Option<String>,
            }
            let CreateFunctionHandleArgs {
                name: fn_name,
                function_handle,
                reference,
            } = with_argument_error("createFunctionHandle", || {
                Ok(serde_json::from_value(args)?)
            })?;
            let function_path = match function_handle {
                Some(fh) => return Ok(serde_json::to_string(&fh)?),
                None => {
                    let reference =
                        with_argument_error("createFunctionHandle", || {
                            match (fn_name, reference) {
                                (Some(name), _) => Ok(Reference::Function(name.parse()?)),
                                (_, Some(reference)) => Ok(reference.parse()?),
                                _ => anyhow::bail!("Missing required argument 'name'"),
                            }
                        })?;
                    match reference {
                        Reference::Function(udf_path) => CanonicalizedComponentFunctionPath {
                            component: ComponentPath::root(),
                            udf_path,
                        },
                        _ => {
                            anyhow::bail!(ErrorMetadata::bad_request(
                                "InvalidReference",
                                "Only function references are supported for function handles"
                            ));
                        },
                    }
                },
            };
            let handle = action_callbacks
                .create_function_handle(identity.clone(), function_path)
                .await?;
            Ok(serde_json::to_string(&String::from(handle))?)
        },
        _ => {
            anyhow::bail!(ErrorMetadata::bad_request(
                "UnknownAsyncOperation",
                format!("Unknown action async operation: {name}")
            ))
        },
    }
}

/// Result of an async op execution, including the op completion and any
/// stream data that should be delivered to V8 before the promise resolves.
struct AsyncOpResult {
    result: anyhow::Result<TaskResponseEnum>,
    stream_parts: Vec<(uuid::Uuid, anyhow::Result<Option<bytes::Bytes>>)>,
}

/// Execute an async op for an action.
async fn handle_action_async_op<RT: Runtime>(
    rt: &RT,
    request: AsyncOpRequest,
    identity: &Identity,
    component_id: ComponentId,
    action_callbacks: &Arc<dyn ActionCallbacks>,
    fetch_client: &Arc<dyn FetchClient>,
    file_storage: &TransactionalFileStorage<RT>,
) -> AsyncOpResult {
    match request {
        AsyncOpRequest::Sleep { until, .. } => {
            let now = rt.unix_timestamp();
            if until > now {
                if let (Ok(until_ms), Ok(now_ms)) =
                    (until.as_ms_since_epoch(), now.as_ms_since_epoch())
                {
                    let duration = Duration::from_millis(until_ms.saturating_sub(now_ms));
                    rt.wait(duration).await;
                }
            }
            AsyncOpResult {
                result: Ok(TaskResponseEnum::Sleep(rt.unix_timestamp())),
                stream_parts: vec![],
            }
        },
        AsyncOpRequest::Fetch {
            request,
            response_body_stream_id,
        } => {
            use crate::http::HttpResponseV8;
            use futures::StreamExt;

            let response = fetch_client.fetch(request).await;
            match response {
                Ok(response) => {
                    match HttpResponseV8::from_response_stream(
                        response,
                        response_body_stream_id,
                    ) {
                        Ok((body, response_v8)) => {
                            let mut stream_parts = vec![];
                            if let Some(mut body) = body {
                                while let Some(chunk) = body.next().await {
                                    stream_parts.push((response_body_stream_id, chunk.map(Some)));
                                }
                            }
                            stream_parts.push((response_body_stream_id, Ok(None)));
                            AsyncOpResult {
                                result: Ok(TaskResponseEnum::Fetch(response_v8)),
                                stream_parts,
                            }
                        },
                        Err(e) => AsyncOpResult {
                            result: Err(e),
                            stream_parts: vec![],
                        },
                    }
                },
                Err(e) => AsyncOpResult {
                    result: Err(e),
                    stream_parts: vec![],
                },
            }
        },
        AsyncOpRequest::StorageGet {
            storage_id,
            stream_id,
        } => {
            use crate::environment::helpers::{
                with_argument_error,
                ArgName,
            };
            use futures::StreamExt;

            let storage_id_parsed: anyhow::Result<FileStorageId> =
                with_argument_error("storage.get", || {
                    storage_id.parse().context(ArgName("storageId"))
                });
            let storage_id_parsed = match storage_id_parsed {
                Ok(id) => id,
                Err(e) => {
                    return AsyncOpResult {
                        result: Err(e),
                        stream_parts: vec![],
                    };
                },
            };
            let entry = match action_callbacks
                .storage_get_file_entry(identity.clone(), component_id, storage_id_parsed)
                .await
            {
                Ok(Some((component_path, file_entry))) => {
                    Some((component_path, file_entry))
                },
                Ok(None) => None,
                Err(e) => {
                    return AsyncOpResult {
                        result: Err(e),
                        stream_parts: vec![],
                    };
                },
            };
            match entry {
                None => AsyncOpResult {
                    result: Ok(TaskResponseEnum::StorageGet(None)),
                    stream_parts: vec![],
                },
                Some((component_path, file_entry)) => {
                    let content_length = file_entry.size as u64;
                    let content_type =
                        file_entry.content_type.as_ref().map(|ct| ct.to_string());
                    // Get the file stream.
                    let file_stream = match file_storage
                        .get_file_stream(
                            component_path,
                            file_entry,
                            usage_tracking::FunctionUsageTracker::new(),
                        )
                        .await
                    {
                        Ok(fs) => fs,
                        Err(e) => {
                            return AsyncOpResult {
                                result: Err(e),
                                stream_parts: vec![],
                            };
                        },
                    };
                    // Buffer the file content into stream_parts.
                    let mut stream_parts = vec![];
                    let mut body = file_stream.stream;
                    while let Some(chunk) = body.next().await {
                        stream_parts.push((
                            stream_id,
                            chunk.map(Some).map_err(|e| e.into()),
                        ));
                    }
                    stream_parts.push((stream_id, Ok(None)));

                    let response = crate::environment::action::FileResponse {
                        body_stream_id: stream_id,
                        content_length,
                        content_type,
                    };
                    AsyncOpResult {
                        result: Ok(TaskResponseEnum::StorageGet(Some(response))),
                        stream_parts,
                    }
                },
            }
        },
        AsyncOpRequest::StorageStore {
            body_stream,
            content_type,
            content_length,
            digest: _,
        } => {
            use headers::{
                ContentLength,
                ContentType,
            };
            use std::str::FromStr;

            let content_length = match content_length
                .map(|c| -> anyhow::Result<ContentLength> { Ok(ContentLength(c.parse()?)) })
                .transpose()
            {
                Ok(cl) => cl,
                Err(e) => {
                    return AsyncOpResult {
                        result: Err(ErrorMetadata::bad_request(
                            "InvalidContentLengthHeader",
                            e.to_string(),
                        )
                        .into()),
                        stream_parts: vec![],
                    };
                },
            };
            let content_type_parsed = match content_type
                .as_ref()
                .map(|c| -> anyhow::Result<ContentType> {
                    Ok(ContentType::from(mime::Mime::from_str(c)?))
                })
                .transpose()
            {
                Ok(ct) => ct,
                Err(e) => {
                    return AsyncOpResult {
                        result: Err(ErrorMetadata::bad_request(
                            "InvalidContentTypeHeader",
                            e.to_string(),
                        )
                        .into()),
                        stream_parts: vec![],
                    };
                },
            };
            // Upload the file.
            let entry = match file_storage
                .upload_file(
                    content_length,
                    content_type_parsed,
                    body_stream.into_stream(),
                    None, // no digest verification for now
                )
                .await
            {
                Ok(entry) => entry,
                Err(e) => {
                    return AsyncOpResult {
                        result: Err(e),
                        stream_parts: vec![],
                    };
                },
            };
            // Persist metadata.
            match action_callbacks
                .storage_store_file_entry(identity.clone(), component_id, entry)
                .await
            {
                Ok((_component_path, doc_id)) => AsyncOpResult {
                    result: Ok(TaskResponseEnum::StorageStore(doc_id)),
                    stream_parts: vec![],
                },
                Err(e) => AsyncOpResult {
                    result: Err(e),
                    stream_parts: vec![],
                },
            }
        },
        AsyncOpRequest::ParseMultiPart {
            content_type,
            request_stream,
        } => {
            use crate::environment::action::{
                FormPart,
                FormPartFile,
            };
            use futures::TryStreamExt;

            // Buffer the entire stream first.
            let body_result: anyhow::Result<Vec<u8>> = async {
                let mut buf = vec![];
                let mut stream = request_stream;
                while let Some(chunk) = stream.try_next().await? {
                    buf.extend_from_slice(&chunk);
                }
                Ok(buf)
            }
            .await;
            let body = match body_result {
                Ok(b) => b,
                Err(e) => {
                    return AsyncOpResult {
                        result: Err(e),
                        stream_parts: vec![],
                    };
                },
            };

            // Parse multipart from the buffer.
            let parse_result: anyhow::Result<Vec<FormPart>> = async {
                let boundary = multer::parse_boundary(&content_type).with_context(|| {
                    ErrorMetadata::bad_request(
                        "InvalidMultiPartForm",
                        format!("multi-part form invalid boundary: '{content_type}'"),
                    )
                })?;
                let stream =
                    futures::stream::once(async { Ok::<_, std::io::Error>(bytes::Bytes::from(body)) });
                let mut multipart = multer::Multipart::with_constraints(
                    stream,
                    boundary,
                    multer::Constraints::new()
                        .size_limit(multer::SizeLimit::new().whole_stream(20 << 20)),
                );
                let mut results = vec![];
                while let Some(field) = multipart.next_field().await? {
                    let name = field
                        .name()
                        .context("multi-part form entry missing 'name'")?
                        .to_string();
                    let (file, text) = match field.file_name() {
                        None => (None, Some(field.text().await?)),
                        Some(file_name) => {
                            let file_name = Some(file_name.to_string());
                            let content_type = field.content_type().map(|c| c.to_string());
                            let data = field.bytes().await?.to_vec().into();
                            (
                                Some(FormPartFile {
                                    content_type,
                                    file_name,
                                    data,
                                }),
                                None,
                            )
                        },
                    };
                    results.push(FormPart { name, text, file });
                }
                Ok(results)
            }
            .await;

            AsyncOpResult {
                result: parse_result.map(TaskResponseEnum::ParseMultiPart),
                stream_parts: vec![],
            }
        },
        AsyncOpRequest::SendStream { .. } => {
            // SendStream is for sending data from Rust to JS. In isolate2's
            // buffered model, stream data is delivered via stream_parts, so
            // this op is a no-op.
            AsyncOpResult {
                result: Ok(TaskResponseEnum::Sleep(rt.unix_timestamp())),
                stream_parts: vec![],
            }
        },
    }
}

// ---------------------------------------------------------------------------
// HTTP Action support
// ---------------------------------------------------------------------------

/// Execute an HTTP action using isolate2.
pub async fn run_isolate_v2_http_action<RT: Runtime>(
    rt: RT,
    mut tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    http_module_path: CanonicalizedUdfPath,
    routed_path: common::http::RoutedHttpPath,
    http_request: udf::HttpActionRequest,
    system_env_vars: BTreeMap<EnvVarName, EnvVarValue>,
    identity: Identity,
    action_callbacks: Arc<dyn ActionCallbacks>,
    fetch_client: Arc<dyn FetchClient>,
    file_storage: TransactionalFileStorage<RT>,
    log_line_sender: tokio::sync::mpsc::UnboundedSender<LogLine>,
    http_response_streamer: udf::HttpActionResponseStreamer,
    context: ExecutionContext,
) -> anyhow::Result<udf::HttpActionOutcome> {
    initialize_v8();

    let semaphore = Arc::new(Semaphore::new(8));
    let user_timeout = *ACTION_USER_TIMEOUT;
    let total_timeout = Duration::from_secs(128);

    let udf_config = UdfConfigModel::new(&mut tx, ComponentId::Root.into())
        .get()
        .await?;
    let import_time_seed = SeedData {
        rng_seed: udf_config
            .as_ref()
            .map(|c| c.import_phase_rng_seed)
            .context("Missing import phase RNG seed")?,
        unix_timestamp: udf_config
            .as_ref()
            .map(|c| c.import_phase_unix_timestamp)
            .context("Missing import phase unix timestamp")?,
    };
    let env_vars = EnvironmentVariablesModel::new(&mut tx).get_all().await?;

    let (action_log_line_sender, action_log_line_receiver) = spsc::channel(32);
    let environment = Isolate2ActionEnvironment::new(
        rt.clone(),
        import_time_seed,
        env_vars,
        system_env_vars,
        action_log_line_sender,
    );

    let (sender, receiver) = mpsc::channel(1);
    let v8_handle = rt.spawn_thread("isolate2_http_action", || async {
        if let Err(e) = v8_thread(receiver, Box::new(environment)).await {
            println!("Error in isolate2 http action thread: {e:?}");
        }
    });

    let client = IsolateThreadClient::new(rt.clone(), sender, user_timeout, semaphore);
    let (result_sender, result_receiver) = oneshot::channel();
    let tokio_handle = rt.spawn(
        "tokio_thread_http_action",
        run_http_action_tokio_thread(
            rt.clone(),
            tx,
            module_loader,
            client,
            total_timeout,
            result_sender,
            http_module_path,
            routed_path,
            http_request,
            action_log_line_receiver,
            identity,
            action_callbacks,
            fetch_client,
            file_storage,
            log_line_sender,
            http_response_streamer,
            context,
        ),
    );

    let r = result_receiver.await??;
    tokio_handle.join().await?;
    v8_handle.join().await?;
    Ok(r)
}

async fn run_http_action_tokio_thread<RT: Runtime>(
    rt: RT,
    mut tx: Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    mut client: IsolateThreadClient<RT>,
    total_timeout: Duration,
    mut sender: oneshot::Sender<anyhow::Result<udf::HttpActionOutcome>>,
    http_module_path: CanonicalizedUdfPath,
    routed_path: common::http::RoutedHttpPath,
    http_request: udf::HttpActionRequest,
    log_line_receiver: spsc::Receiver<LogLine>,
    identity: Identity,
    action_callbacks: Arc<dyn ActionCallbacks>,
    fetch_client: Arc<dyn FetchClient>,
    file_storage: TransactionalFileStorage<RT>,
    log_line_sender: tokio::sync::mpsc::UnboundedSender<LogLine>,
    http_response_streamer: udf::HttpActionResponseStreamer,
    context: ExecutionContext,
) {
    let request = run_http_action_request(
        rt.clone(),
        &mut tx,
        module_loader,
        &mut client,
        http_module_path,
        routed_path,
        http_request,
        log_line_receiver,
        identity,
        action_callbacks,
        fetch_client,
        file_storage,
        log_line_sender,
        http_response_streamer,
        context,
    );
    let r = tokio::select! {
        r = request => r,
        _ = rt.wait(total_timeout) => Err(anyhow::anyhow!("Total timeout exceeded")),
        _ = sender.closed() => Err(anyhow::anyhow!("Cancelled")),
    };
    let _ = sender.send(r);
    drop(client);
}

async fn run_http_action_request<RT: Runtime>(
    rt: RT,
    tx: &mut Transaction<RT>,
    module_loader: Arc<dyn ModuleLoader<RT>>,
    client: &mut IsolateThreadClient<RT>,
    http_module_path: CanonicalizedUdfPath,
    routed_path: common::http::RoutedHttpPath,
    http_request: udf::HttpActionRequest,
    mut log_line_receiver: spsc::Receiver<LogLine>,
    identity: Identity,
    action_callbacks: Arc<dyn ActionCallbacks>,
    fetch_client: Arc<dyn FetchClient>,
    file_storage: TransactionalFileStorage<RT>,
    log_line_sender: tokio::sync::mpsc::UnboundedSender<LogLine>,
    mut http_response_streamer: udf::HttpActionResponseStreamer,
    context: ExecutionContext,
) -> anyhow::Result<udf::HttpActionOutcome> {
    use crate::http::HttpResponseV8;
    use super::client::HttpActionStartResult;

    let component_id = ComponentId::Root;
    let route_for_failure: common::types::HttpActionRoute =
        http_request.head.route_for_failure();

    // Spawn log line forwarder.
    let (log_done_tx, log_done_rx) = oneshot::channel();
    let fwd_sender = log_line_sender.clone();
    let log_forwarder = rt.spawn("http_action_log_forwarder", async move {
        while let Some(line) = log_line_receiver.recv().await {
            let _ = fwd_sender.send(line);
        }
        let _ = log_done_tx.send(());
    });

    let http_module = http_module_path.module().clone();
    let r: anyhow::Result<_> = try {
        let mut stack = vec![http_module.clone()];
        while let Some(module_path) = stack.pop() {
            let module_specifier = module_specifier_from_path(&module_path)?;
            let component_module_path = CanonicalizedComponentModulePath {
                component: component_id,
                module_path: module_path.clone(),
            };
            let Some(module_metadata) =
                module_loader.get_module(tx, component_module_path).await?
            else {
                let err = ModuleNotFoundError::new(module_path.as_str());
                Err(JsError::from_message(format!("{err}")))?
            };
            let requests = client
                .register_module(
                    module_specifier,
                    module_metadata.source.to_string(),
                    module_metadata.source_map.clone(),
                )
                .await?;
            for specifier in requests {
                stack.push(path_from_module_specifier(&specifier)?);
            }
        }
        let module_specifier = module_specifier_from_path(&http_module)?;
        client.evaluate_module(module_specifier).await?;
    };
    if let Err(e) = r {
        let js_error = e.downcast::<JsError>()?;
        client.shutdown().await?;
        log_forwarder.join().await?;
        let _ = log_done_rx.await;
        return Ok(udf::HttpActionOutcome::new(
            Some(route_for_failure.clone()),
            http_request.head.clone(),
            tx.inert_identity(),
            rt.unix_timestamp(),
            udf::HttpActionResult::Error(js_error),
            None,
            None,
            Duration::ZERO,
        ));
    }

    // Buffer the request body if present.
    let body_bytes: Option<bytes::Bytes> = if let Some(mut body) = http_request.body {
        use futures::TryStreamExt;
        let mut buf = vec![];
        while let Some(chunk) = body.try_next().await? {
            buf.extend_from_slice(&chunk);
        }
        Some(buf.into())
    } else {
        None
    };

    // Build the request JSON (signal and body stream_id will be set by V8 thread).
    let signal_placeholder = uuid::Uuid::nil();
    let request_v8 = crate::http::HttpRequestV8::from_request(
        http_request.head.clone(),
        None, // Body stream_id set by V8 thread.
        signal_placeholder,
    )?;
    let request_json = serde_json::to_value(&request_v8)?.to_string();

    let start_result = client
        .start_http_action(
            http_module_path.clone(),
            routed_path.0.clone(),
            request_json,
            http_request.head.method.to_string(),
            body_bytes,
        )
        .await?;

    let (function_id, mut result, route) = match start_result {
        HttpActionStartResult::Started {
            function_id,
            result,
            route,
        } => (function_id, result, route),
        HttpActionStartResult::NoRoute => {
            // Send 404 response.
            let head = udf::HttpActionResponseHead {
                status: http::StatusCode::NOT_FOUND,
                headers: http::HeaderMap::new(),
            };
            let _ = http_response_streamer.send_part(udf::HttpActionResponsePart::Head(head));
            let _ = http_response_streamer.send_part(
                udf::HttpActionResponsePart::BodyChunk(bytes::Bytes::from(
                    "No matching routes found",
                )));
            client.shutdown().await?;
            log_forwarder.join().await?;
            let _ = log_done_rx.await;
            return Ok(udf::HttpActionOutcome::new(
                Some(route_for_failure.clone()),
                http_request.head,
                tx.inert_identity(),
                rt.unix_timestamp(),
                udf::HttpActionResult::Streamed,
                None,
                None,
                Duration::ZERO,
            ));
        },
        HttpActionStartResult::Error(e) => {
            let _ = client.shutdown().await;
            log_forwarder.join().await?;
            let _ = log_done_rx.await;
            return Ok(udf::HttpActionOutcome::new(
                Some(route_for_failure.clone()),
                http_request.head,
                tx.inert_identity(),
                rt.unix_timestamp(),
                udf::HttpActionResult::Error(e),
                None,
                None,
                Duration::ZERO,
            ));
        },
    };

    let r: anyhow::Result<_> = try {
        loop {
            let pending = match result {
                EvaluateResult::Ready(r) => break r,
                EvaluateResult::Pending(p) => p,
            };
            let mut completions = Completions::new();

            for PendingAsyncSyscall {
                promise_id,
                name,
                args,
            } in pending.async_syscalls
            {
                let syscall_result = handle_action_syscall(
                    &rt, &name, args, &identity, component_id, &action_callbacks,
                    &fetch_client, &context,
                )
                .await;
                let syscall_result = syscall_result.map_err(|mut e| {
                    if !e.is_deterministic_user_error() {
                        common::errors::report_error_sync(&mut e);
                        anyhow::anyhow!(JsError::from_message(e.user_facing_message()))
                    } else {
                        e
                    }
                });
                completions.async_syscalls.push(AsyncSyscallCompletion {
                    promise_id,
                    result: syscall_result,
                });
            }

            for async_op in pending.async_ops {
                let op_result = handle_action_async_op(
                    &rt, async_op.request, &identity, component_id,
                    &action_callbacks, &fetch_client, &file_storage,
                )
                .await;
                completions.stream_parts.extend(op_result.stream_parts);
                completions.async_ops.push(AsyncOpCompletion {
                    promise_id: async_op.promise_id,
                    result: op_result.result,
                });
            }

            // Handle dynamic imports in HTTP actions.
            for dynamic_import in pending.dynamic_imports {
                let module_path =
                    path_from_module_specifier(&dynamic_import.specifier)?;
                let component_module_path = CanonicalizedComponentModulePath {
                    component: component_id,
                    module_path,
                };
                if let Some(module_metadata) =
                    module_loader.get_module(tx, component_module_path).await?
                {
                    let requests = client
                        .register_module(
                            dynamic_import.specifier.clone(),
                            module_metadata.source.to_string(),
                            module_metadata.source_map.clone(),
                        )
                        .await?;
                    let mut stack: Vec<_> = requests;
                    while let Some(dep_specifier) = stack.pop() {
                        let dep_path = path_from_module_specifier(&dep_specifier)?;
                        let dep_component_path = CanonicalizedComponentModulePath {
                            component: component_id,
                            module_path: dep_path,
                        };
                        if let Some(dep_metadata) =
                            module_loader.get_module(tx, dep_component_path).await?
                        {
                            let more = client
                                .register_module(
                                    dep_specifier,
                                    dep_metadata.source.to_string(),
                                    dep_metadata.source_map.clone(),
                                )
                                .await?;
                            stack.extend(more);
                        }
                    }
                    client.evaluate_module(dynamic_import.specifier).await?;
                }
            }
            result = client.poll_function(function_id, completions).await?;
        }
    };

    let http_action_result = match r {
        Ok(result_value) => {
            // The result is a JSON string (HttpResponseV8).
            let result_str: String = result_value.try_into()?;
            let response_v8: HttpResponseV8 = serde_json::from_str(&result_str)?;
            let (raw_response, stream_id) = response_v8.into_response()?;

            // Send the response head through the streamer.
            let _ = http_response_streamer.send_part(udf::HttpActionResponsePart::Head(
                udf::HttpActionResponseHead {
                    status: raw_response.status,
                    headers: raw_response.headers,
                },
            ));

            // Extract the response body from the V8 context stream.
            if let Some(stream_id) = stream_id {
                let body_chunks = client.extract_stream(stream_id).await?;
                for chunk in body_chunks {
                    let _ = http_response_streamer
                        .send_part(udf::HttpActionResponsePart::BodyChunk(chunk));
                }
            }
            udf::HttpActionResult::Streamed
        },
        Err(e) => {
            let js_error = match e.downcast::<JsError>() {
                Ok(js_error) => js_error,
                Err(e) => JsError::from_message(e.to_string()),
            };
            udf::HttpActionResult::Error(js_error)
        },
    };

    let _ = client.shutdown().await;
    log_forwarder.join().await?;
    let _ = log_done_rx.await;

    let route_parsed: common::types::HttpActionRoute =
        route.parse().unwrap_or(route_for_failure);
    let user_execution_time = client
        .execution_time()
        .map(|t| t.elapsed)
        .unwrap_or(Duration::ZERO);

    Ok(udf::HttpActionOutcome::new(
        Some(route_parsed),
        http_request.head,
        tx.inert_identity(),
        rt.unix_timestamp(),
        http_action_result,
        None,
        None,
        user_execution_time,
    ))
}
