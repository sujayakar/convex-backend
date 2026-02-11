use anyhow::anyhow;
use common::{
    components::{
        ComponentId,
        ComponentPath,
        ResolvedComponentFunctionPath,
    },
    errors::JsError,
    types::UdfType,
};
use deno_core::{
    serde_v8,
    v8::{
        self,
        scope,
        tc_scope,
    },
    ModuleSpecifier,
};
use model::modules::user_error::{
    FunctionNotFoundError,
    ModuleNotFoundError,
};
use sync_types::CanonicalizedUdfPath;
use value::ConvexArray;

use super::{
    client::{
        Completions,
        EvaluateResult,
    },
    context::PendingFunction,
    context_state::ContextState,
    environment::EnvironmentOutcome,
    session::HeapContext,
};
use crate::{
    deserialize_udf_result,
    environment::helpers::{
        module_loader::module_specifier_from_path,
        resolve_promise,
    },
    error::extract_source_mapped_error,
    helpers::{
        self,
        pump_message_loop,
        source_map_from_slice,
        to_rust_string,
    },
    isolate2::{
        callback_context::CallbackContext,
        context_state::ContextFailure,
    },
    metrics::{
        self,
        log_isolate_out_of_memory,
    },
    strings::{
        self,
        StaticString,
    },
    termination::OutOfMemoryError,
};

pub struct EnteredContext<'enter, 'scope: 'enter, 'i> {
    scope: &'enter mut v8::PinScope<'scope, 'i>,
    heap_context: &'enter HeapContext,
    _context: v8::Local<'scope, v8::Context>,
}

impl<'enter, 'scope: 'enter, 'i> EnteredContext<'enter, 'scope, 'i> {
    pub fn new(
        scope: &'enter mut v8::PinScope<'scope, 'i>,
        heap_context: &'enter HeapContext,
        context: v8::Local<'scope, v8::Context>,
    ) -> Self {
        Self {
            scope,
            heap_context,
            _context: context,
        }
    }

    pub fn context_state_mut(&mut self) -> anyhow::Result<&mut ContextState> {
        self.scope
            .get_slot_mut::<ContextState>()
            .ok_or_else(|| anyhow::anyhow!("ContextState not found in context"))
    }

    pub fn context_state(&mut self) -> anyhow::Result<&ContextState> {
        self.scope
            .get_slot::<ContextState>()
            .ok_or_else(|| anyhow::anyhow!("ContextState not found in context"))
    }

    // NB: This can be called from the top-level (i.e. entering from the context
    // into user code) but also from within a callback (e.g. following an object
    // property in an op handler).
    pub fn execute_user_code<R>(
        &mut self,
        f: impl FnOnce(&mut v8::PinScope<'scope, 'i>) -> R,
    ) -> anyhow::Result<R> {
        let (r, terminated, exception);
        {
            tc_scope!(let tc_scope, self.scope);
            r = f(tc_scope);
            terminated = tc_scope.has_terminated();
            exception = tc_scope.exception();
        }
        if terminated {
            let context_state = self.context_state_mut()?;
            if let Some(failure) = context_state.failure.take() {
                match failure {
                    ContextFailure::UncatchableDeveloperError(js_error) => anyhow::bail!(js_error),
                    ContextFailure::SystemError(error) => {
                        #[cfg(test)]
                        let error = match error.downcast::<crate::test_helpers::PanicError>() {
                            Ok(panic) => std::panic::resume_unwind(panic.into_inner()),
                            Err(e) => e,
                        };
                        anyhow::bail!(error)
                    },
                }
            } else if self.heap_context.oomed() {
                log_isolate_out_of_memory();
                let stats = self.scope.get_heap_statistics();
                tracing::warn!(
                    used_heap_size = stats.used_heap_size(),
                    total_heap_size = stats.total_heap_size(),
                    heap_size_limit = stats.heap_size_limit(),
                    "isolate2: out of memory"
                );
                anyhow::bail!(JsError::from_message(format!("{OutOfMemoryError}")));
            } else {
                anyhow::bail!("Execution terminated");
            }
        }
        if let Some(e) = exception {
            return Err(self.format_traceback(e)?.into());
        }
        // Executing just about any user code can lead to an unhandled promise
        // rejection (e.g. calling `Promise.reject`). However, it's important
        // to only fail the session when we receive control... XXX explain more.
        let promise_rejection = {
            let context_state = self.context_state_mut()?;

            // Only use the first unhandled promise rejection.
            let rejection = context_state.unhandled_promise_rejections.drain().next();
            context_state.unhandled_promise_rejections.clear();
            rejection
        };
        if let Some((_promise, error_global)) = promise_rejection {
            let error = v8::Local::new(self.scope, error_global);
            let err = self.format_traceback(error)?;
            return Err(err.into());
        }
        Ok(r)
    }

    pub fn register_module(
        &mut self,
        url: &ModuleSpecifier,
        source: &str,
        source_map: Option<String>,
    ) -> anyhow::Result<Vec<ModuleSpecifier>> {
        {
            let context_state = self.context_state_mut()?;
            anyhow::ensure!(
                !context_state.module_map.contains_module(url),
                "Module already registered"
            );
        }
        let name_str = v8::String::new(self.scope, url.as_str())
            .ok_or_else(|| anyhow!("Failed to create name string"))?;
        let source_str = v8::String::new(self.scope, source)
            .ok_or_else(|| anyhow!("Failed to create source string"))?;

        let origin = helpers::module_origin(self.scope, name_str);
        let mut v8_source = v8::script_compiler::Source::new(source_str, Some(&origin));

        let module = self
            .execute_user_code(|s| v8::script_compiler::compile_module(s, &mut v8_source))?
            .ok_or_else(|| anyhow!("Unexpected module compilation error"))?;

        anyhow::ensure!(module.get_status() == v8::ModuleStatus::Uninstantiated);
        let mut import_specifiers: Vec<ModuleSpecifier> = vec![];
        let module_requests = module.get_module_requests();
        for i in 0..module_requests.length() {
            let module_request: v8::Local<v8::ModuleRequest> = module_requests
                .get(self.scope, i)
                .ok_or_else(|| anyhow!("Module request {} out of bounds", i))?
                .try_into()?;
            let import_specifier =
                helpers::to_rust_string(self.scope, &module_request.get_specifier())?;
            let module_specifier = deno_core::resolve_import(&import_specifier, url.as_str())?;
            import_specifiers.push(module_specifier);
        }
        let module = v8::Global::new(self.scope, module);
        let unresolved_imports = {
            let context_state = self.context_state_mut()?;
            import_specifiers.retain(|s| !context_state.module_map.contains_module(s));
            context_state
                .module_map
                .register(url.clone(), module, source_map)?;
            import_specifiers
        };
        Ok(unresolved_imports)
    }

    pub fn evaluate_module(
        &mut self,
        url: &ModuleSpecifier,
    ) -> anyhow::Result<v8::Local<'enter, v8::Module>> {
        let module_global = {
            let context_state = self.context_state()?;
            context_state
                .module_map
                .lookup_module(url)
                .ok_or_else(|| anyhow!("Module not registered"))?
                .clone()
        };
        let module = v8::Local::new(self.scope, module_global);
        match module.get_status() {
            v8::ModuleStatus::Uninstantiated => (),
            s => anyhow::bail!("Module {url} is in invalid state: {s:?}"),
        }

        let instantiation_result = self
            .execute_user_code(|s| module.instantiate_module(s, CallbackContext::resolve_module))?;

        if matches!(instantiation_result, Some(false) | None) {
            anyhow::bail!("Unexpected successful instantiate result: {instantiation_result:?}");
        }
        anyhow::ensure!(module.get_status() == v8::ModuleStatus::Instantiated);

        let evaluation_result = self
            .execute_user_code(|s| module.evaluate(s))?
            .ok_or_else(|| anyhow!("Missing result from successful module evaluation"))?;

        let status = module.get_status();
        anyhow::ensure!(
            status == v8::ModuleStatus::Evaluated || status == v8::ModuleStatus::Errored
        );
        let promise = v8::Local::<v8::Promise>::try_from(evaluation_result)
            .map_err(|e| anyhow!("Module evaluation did not return a promise: {:?}", e))?;
        match promise.state() {
            v8::PromiseState::Pending => {
                anyhow::bail!(JsError::from_message(
                    "Top-level awaits in source files are unsupported".to_string()
                ))
            },
            v8::PromiseState::Fulfilled => {
                anyhow::ensure!(status == v8::ModuleStatus::Evaluated);
            },
            v8::PromiseState::Rejected => {
                let e = promise.result(self.scope);
                return Err(self.format_traceback(e)?.into());
            },
        }
        Ok(module)
    }

    pub fn start_evaluate_function(
        &mut self,
        udf_type: UdfType,
        udf_path: &CanonicalizedUdfPath,
        arguments: ConvexArray,
    ) -> anyhow::Result<(v8::Global<v8::Promise>, EvaluateResult)> {
        let module_url = module_specifier_from_path(udf_path.module())?;
        let module_global = {
            let context_state = self.context_state_mut()?;
            context_state.environment.start_execution()?;
            context_state
                .module_map
                .lookup_module(&module_url)
                .ok_or_else(|| {
                    let err = ModuleNotFoundError::new(udf_path.module().as_str());
                    JsError::from_message(err.to_string())
                })?
                .clone()
        };
        let module = v8::Local::new(self.scope, module_global);
        match module.get_status() {
            v8::ModuleStatus::Evaluated => (),
            s => anyhow::bail!("Module is in invalid state: {s:?}"),
        }

        let exports = module.get_module_namespace();
        let exports = v8::Local::new(self.scope, exports);
        let exports = exports
            .to_object(self.scope)
            .ok_or_else(|| anyhow!("Module exports not an object"))?;

        let name_str = v8::String::new(self.scope, udf_path.function_name())
            .ok_or_else(|| anyhow::anyhow!("Failed to create name string"))?
            .into();
        if exports.has(self.scope, name_str) != Some(true) {
            let err =
                FunctionNotFoundError::new(udf_path.function_name(), udf_path.module().as_str());
            anyhow::bail!(JsError::from_message(err.to_string()));
        }
        let function: v8::Local<v8::Object> = exports
            .get(self.scope, name_str)
            .ok_or_else(|| {
                let err = FunctionNotFoundError::new(
                    udf_path.function_name(),
                    udf_path.module().as_str(),
                );
                JsError::from_message(err.to_string())
            })?
            .try_into()?;
        let invoke_str = self.classify_function(udf_type, udf_path, &function)?;

        let args_str = arguments.json_serialize()?;
        let args_v8_str = v8::String::new(self.scope, &args_str)
            .ok_or_else(|| anyhow!("Failed to create argument string"))?;

        let invoke: v8::Local<v8::Function> = function
            .get(self.scope, invoke_str.into())
            .ok_or_else(|| {
                let msg = format!("Couldn't find invoke function in {udf_path:?}");
                JsError::from_message(msg)
            })?
            .try_into()?;

        let global = self.scope.get_current_context().global(self.scope);

        // Actions take (requestId, argsStr) while queries/mutations take (argsStr).
        let v8_args: Vec<v8::Local<v8::Value>> = if udf_type == UdfType::Action {
            let request_id_str = v8::String::new(self.scope, "dummy_request_id")
                .ok_or_else(|| anyhow!("Failed to create request id string"))?;
            vec![request_id_str.into(), args_v8_str.into()]
        } else {
            vec![args_v8_str.into()]
        };

        let promise: v8::Local<v8::Promise> = self
            .execute_user_code(|scope| invoke.call(scope, global.into(), &v8_args))?
            .ok_or_else(|| anyhow!("Failed to call invoke function"))?
            .try_into()?;

        // Calling into our function can put entries into the microtask queue, so
        // ensure that the microtask queue is clean before returning to the Tokio
        // thread. This ensure that we've driven the promise as far as possible
        // before collecting what it's blocked on.
        self.execute_user_code(|s| s.perform_microtask_checkpoint())?;
        pump_message_loop(self.scope);

        let path = ResolvedComponentFunctionPath {
            component: ComponentId::Root,
            udf_path: udf_path.clone(),
            component_path: Some(ComponentPath::root()),
        };
        let evaluate_result = self.check_promise_result(&path, &promise)?;
        Ok((v8::Global::new(self.scope, promise), evaluate_result))
    }

    fn classify_function(
        &mut self,
        udf_type: UdfType,
        udf_path: &CanonicalizedUdfPath,
        function: &v8::Local<v8::Object>,
    ) -> anyhow::Result<v8::Local<'scope, v8::String>> {
        let is_query = self.classify_function_object(&strings::isQuery, function)?;
        let is_mutation = self.classify_function_object(&strings::isMutation, function)?;
        let is_action = self.classify_function_object(&strings::isAction, function)?;

        let invoke_str = match (udf_type, is_query, is_mutation, is_action) {
            (UdfType::Query, true, false, false) => strings::invokeQuery.create(self.scope)?,
            (UdfType::Mutation, false, true, false) => {
                strings::invokeMutation.create(self.scope)?
            },
            (UdfType::Query, false, true, _) => {
                let message = format!(
                    "Function {udf_path:?} is registered as a mutation but is being run as a \
                     query."
                );
                anyhow::bail!(JsError::from_message(message));
            },
            (UdfType::Mutation, true, false, _) => {
                let message = format!(
                    "Function {udf_path:?} is registered as a query but is being run as a \
                     mutation."
                );
                anyhow::bail!(JsError::from_message(message));
            },
            (UdfType::Action, false, false, true) => {
                strings::invokeAction.create(self.scope)?
            },
            (UdfType::Action, _, _, false) => {
                let message = format!(
                    "Function {udf_path:?} is not registered as an action. Did you forget to wrap \
                     it with `action`?"
                );
                anyhow::bail!(JsError::from_message(message));
            },
            (UdfType::Query | UdfType::Mutation, false, false, _) => {
                let message = format!(
                    "Function {udf_path:?} is neither a query or mutation. Did you forget to wrap \
                     it with `query` or `mutation`?"
                );
                anyhow::bail!(JsError::from_message(message));
            },
            _ => {
                anyhow::bail!(
                    "Unexpected function classification: {udf_type} vs. (is_query: {is_query}, \
                     is_mutation: {is_mutation}, is_action: {is_action})"
                );
            },
        };
        Ok(invoke_str)
    }

    fn classify_function_object(
        &mut self,
        function_type: &'static StaticString,
        function: &v8::Local<v8::Object>,
    ) -> anyhow::Result<bool> {
        let function_type_str = function_type.create(self.scope)?.into();
        let has_function_type = function.has(self.scope, function_type_str) == Some(true);
        let is_function_type = has_function_type
            && function
                .get(self.scope, function_type_str)
                .ok_or_else(|| anyhow!("Failed to get {} property", function_type.rust_str()))?
                .is_true();
        Ok(is_function_type)
    }

    pub fn poll_function(
        &mut self,
        pending_function: &PendingFunction,
        completions: Completions,
    ) -> anyhow::Result<EvaluateResult> {
        let stream_parts = completions.stream_parts;
        let (async_syscalls, async_ops) = {
            let context_state = self.context_state_mut()?;
            let mut async_syscalls = vec![];
            for completion in completions.async_syscalls {
                let resolver = context_state.take_promise(completion.promise_id)?;
                async_syscalls.push((resolver, completion.result));
            }
            let mut async_ops = vec![];
            for completion in completions.async_ops {
                let resolver = context_state.take_promise(completion.promise_id)?;
                async_ops.push((resolver, completion.result));
            }
            (async_syscalls, async_ops)
        };
        for (resolver, result) in async_syscalls {
            scope!(let scope, self.scope);
            let result_v8 = match result {
                Ok(v) => Ok(serde_v8::to_v8(scope, v)?),
                Err(e) => Err(e),
            };
            resolve_promise(scope, resolver, result_v8)?;
        }
        // Deliver stream data before resolving async op promises so stream
        // content is available when the promise handler runs (e.g., for fetch
        // response bodies).
        for (stream_id, chunk) in stream_parts {
            let context_state = self.context_state_mut()?;
            match chunk {
                Ok(data) => {
                    let done = data.is_none();
                    context_state.extend_stream(stream_id, data, done)?;
                },
                Err(_e) => {
                    // Stream error: mark the stream as done with no more data.
                    context_state.extend_stream(stream_id, None, true)?;
                },
            }
        }

        // Resolve each async op completion individually with microtask
        // checkpoints between each. This ensures timer callbacks and their
        // microtasks execute in the correct order (matching isolate1 behavior
        // where each task response is processed one at a time).
        for (resolver, result) in async_ops {
            {
                scope!(let scope, self.scope);
                let result_v8 = match result {
                    Ok(v) => Ok(v.into_v8(scope)?),
                    Err(e) => Err(e),
                };
                resolve_promise(scope, resolver, result_v8)?;
            }
            self.execute_user_code(|s| s.perform_microtask_checkpoint())?;
            pump_message_loop(self.scope);
        }

        self.execute_user_code(|s| s.perform_microtask_checkpoint())?;
        pump_message_loop(self.scope);

        let promise = v8::Local::new(self.scope, &pending_function.promise);
        let path = ResolvedComponentFunctionPath {
            component: ComponentId::Root,
            udf_path: pending_function.udf_path.clone(),
            component_path: Some(ComponentPath::root()),
        };
        if pending_function.is_http_action {
            self.check_http_promise_result(&path, &promise)
        } else {
            self.check_promise_result(&path, &promise)
        }
    }

    fn check_promise_result(
        &mut self,
        path: &ResolvedComponentFunctionPath,
        promise: &v8::Local<v8::Promise>,
    ) -> anyhow::Result<EvaluateResult> {
        let context = self.context_state_mut()?;
        let pending = context.take_pending();
        match promise.state() {
            v8::PromiseState::Pending if pending.is_empty() => {
                anyhow::bail!(JsError::from_message(
                    "Returned promise will never resolve".to_string()
                ))
            },
            v8::PromiseState::Rejected => {
                let e = promise.result(self.scope);
                anyhow::bail!(self.format_traceback(e)?);
            },
            v8::PromiseState::Fulfilled
                if pending.is_empty()
                    || (pending.async_syscalls.is_empty()
                        && pending.dynamic_imports.is_empty()) =>
            {
                // Return Ready when the promise is fulfilled, even if there
                // are dangling async ops (timers). This matches isolate1's
                // behavior where the action loop discards remaining tasks
                // once the main promise resolves.
                let v8_result: v8::Local<v8::String> = promise.result(self.scope).try_into()?;
                let result_str = helpers::to_rust_string(self.scope, &v8_result)?;
                let result = deserialize_udf_result(path, &result_str)??;
                Ok(EvaluateResult::Ready(result))
            },
            v8::PromiseState::Pending | v8::PromiseState::Fulfilled => {
                Ok(EvaluateResult::Pending(pending))
            },
        }
    }

    pub fn shutdown(&mut self) -> anyhow::Result<EnvironmentOutcome> {
        let context_state = self.context_state_mut()?;
        let outcome = context_state.environment.finish_execution()?;
        Ok(outcome)
    }

    /// Start an HTTP action by looking up the router in the HTTP module,
    /// calling `router.lookup(path, method)`, and then `router.runRequest(request, route)`.
    pub fn start_http_action(
        &mut self,
        http_module_path: &CanonicalizedUdfPath,
        routed_path: &str,
        request_json: &str,
        method: &str,
        body: Option<bytes::Bytes>,
    ) -> anyhow::Result<super::client::HttpActionStartResultInner> {
        use super::client::HttpActionStartResultInner as HttpActionStartResult;
        use crate::strings;

        {
            let context_state = self.context_state_mut()?;
            context_state.environment.start_execution()?;
        }

        let module_url = module_specifier_from_path(http_module_path.module())?;
        let module_global = {
            let context_state = self.context_state_mut()?;
            context_state
                .module_map
                .lookup_module(&module_url)
                .ok_or_else(|| {
                    let err = ModuleNotFoundError::new(http_module_path.module().as_str());
                    JsError::from_message(err.to_string())
                })?
                .clone()
        };
        let module = v8::Local::new(self.scope, module_global);
        if module.get_status() != v8::ModuleStatus::Evaluated {
            anyhow::bail!("HTTP module not evaluated");
        }
        let namespace = module.get_module_namespace();
        let namespace = v8::Local::new(self.scope, namespace)
            .to_object(self.scope)
            .ok_or_else(|| anyhow!("Module namespace not an object"))?;

        let default_str = v8::String::new(self.scope, "default")
            .ok_or_else(|| anyhow!("Failed to create string"))?;
        if namespace.has(self.scope, default_str.into()) != Some(true) {
            return Ok(HttpActionStartResult::Error(JsError::from_message(
                format!(
                    "Couldn't find default export in module \"{:?}\".",
                    http_module_path.module()
                ),
            )));
        }
        let router: v8::Local<v8::Object> = namespace
            .get(self.scope, default_str.into())
            .ok_or_else(|| anyhow!("Missing default export"))?
            .try_into()?;

        let is_router_str = strings::isRouter.create(self.scope)?.into();
        let is_router = router.has(self.scope, is_router_str) == Some(true)
            && router
                .get(self.scope, is_router_str)
                .map(|v| v.is_true())
                .unwrap_or(false);
        if !is_router {
            return Ok(HttpActionStartResult::Error(JsError::from_message(
                "The default export of `convex/http.js` is not a Router.".to_string(),
            )));
        }

        let lookup_str = strings::lookup.create(self.scope)?.into();
        let lookup_fn: v8::Local<v8::Function> = router
            .get(self.scope, lookup_str)
            .ok_or_else(|| anyhow!("Missing lookup method on router"))?
            .try_into()?;
        let path_v8 = v8::String::new(self.scope, routed_path)
            .ok_or_else(|| anyhow!("Failed to create path string"))?;
        let method_v8 = v8::String::new(self.scope, method)
            .ok_or_else(|| anyhow!("Failed to create method string"))?;
        let global = self.scope.get_current_context().global(self.scope);
        let lookup_result = self.execute_user_code(|scope| {
            lookup_fn.call(scope, global.into(), &[path_v8.into(), method_v8.into()])
        })?;
        let lookup_result = match lookup_result {
            Some(v) if !v.is_null() && !v.is_undefined() => v,
            _ => return Ok(HttpActionStartResult::NoRoute),
        };

        let lookup_obj = lookup_result
            .to_object(self.scope)
            .ok_or_else(|| anyhow!("lookup result not an object"))?;
        let route_method_v8: v8::Local<v8::String> = lookup_obj
            .get_index(self.scope, 1)
            .ok_or_else(|| anyhow!("Missing index 1 in lookup result"))?
            .try_into()?;
        let route_method_s = helpers::to_rust_string(self.scope, &route_method_v8)?;
        let route_path_v8: v8::Local<v8::String> = lookup_obj
            .get_index(self.scope, 2)
            .ok_or_else(|| anyhow!("Missing index 2 in lookup result"))?
            .try_into()?;
        let route_path_s = helpers::to_rust_string(self.scope, &route_path_v8)?;
        let route = format!("{route_method_s} {route_path_s}");

        let (signal_stream_id, body_stream_id) = {
            let context_state = self.context_state_mut()?;
            let signal_id = context_state.create_stream()?;
            let body_id = if body.is_some() {
                let id = context_state.create_stream()?;
                if let Some(body_data) = &body {
                    context_state.extend_stream(id, Some(body_data.clone()), true)?;
                }
                Some(id)
            } else {
                None
            };
            (signal_id, body_id)
        };

        let patched_request_json = {
            let mut request_value: serde_json::Value = serde_json::from_str(request_json)?;
            if let Some(obj) = request_value.as_object_mut() {
                obj.insert(
                    "signal".to_string(),
                    serde_json::Value::String(signal_stream_id.to_string()),
                );
                if let Some(body_id) = body_stream_id {
                    obj.insert(
                        "streamId".to_string(),
                        serde_json::Value::String(body_id.to_string()),
                    );
                }
            }
            request_value.to_string()
        };

        let run_request_str = strings::runRequest.create(self.scope)?.into();
        let run_request_fn: v8::Local<v8::Function> = router
            .get(self.scope, run_request_str)
            .ok_or_else(|| anyhow!("Missing runRequest method on router"))?
            .try_into()?;
        let args_v8 = v8::String::new(self.scope, &patched_request_json)
            .ok_or_else(|| anyhow!("Failed to create request string"))?;
        let route_v8 = v8::String::new(self.scope, routed_path)
            .ok_or_else(|| anyhow!("Failed to create route string"))?;
        let promise: v8::Local<v8::Promise> = self
            .execute_user_code(|scope| {
                run_request_fn.call(scope, global.into(), &[args_v8.into(), route_v8.into()])
            })?
            .ok_or_else(|| anyhow!("runRequest returned None"))?
            .try_into()?;

        self.execute_user_code(|s| s.perform_microtask_checkpoint())?;
        pump_message_loop(self.scope);

        let path = ResolvedComponentFunctionPath {
            component: ComponentId::Root,
            udf_path: http_module_path.clone(),
            component_path: Some(ComponentPath::root()),
        };
        let promise_global = v8::Global::new(self.scope, promise);
        let evaluate_result = self.check_http_promise_result(&path, &promise)?;

        Ok(HttpActionStartResult::Started {
            promise: promise_global,
            udf_path: http_module_path.clone(),
            result: evaluate_result,
            route,
        })
    }

    /// Check the promise result for HTTP actions. Unlike UDFs, the result is
    /// a raw JSON string (not a Convex value), so we don't deserialize it.
    fn check_http_promise_result(
        &mut self,
        _path: &ResolvedComponentFunctionPath,
        promise: &v8::Local<v8::Promise>,
    ) -> anyhow::Result<EvaluateResult> {
        let context = self.context_state_mut()?;
        let pending = context.take_pending();
        match promise.state() {
            v8::PromiseState::Pending if pending.is_empty() => {
                anyhow::bail!(JsError::from_message(
                    "Returned promise will never resolve".to_string()
                ))
            },
            v8::PromiseState::Rejected => {
                let e = promise.result(self.scope);
                anyhow::bail!(self.format_traceback(e)?);
            },
            v8::PromiseState::Fulfilled
                if pending.is_empty()
                    || (pending.async_syscalls.is_empty()
                        && pending.dynamic_imports.is_empty()) =>
            {
                let v8_result: v8::Local<v8::String> = promise.result(self.scope).try_into()?;
                let result_str = helpers::to_rust_string(self.scope, &v8_result)?;
                Ok(EvaluateResult::Ready(
                    value::ConvexValue::try_from(result_str)?,
                ))
            },
            v8::PromiseState::Pending | v8::PromiseState::Fulfilled => {
                Ok(EvaluateResult::Pending(pending))
            },
        }
    }

    pub fn format_traceback(&mut self, exception: v8::Local<v8::Value>) -> anyhow::Result<JsError> {
        // Check if we hit a system error or timeout and can't run any JavaScript now.
        // Abort with a system error here, and we'll (in the best case) pull out
        // the original system error that initiated the termination.
        if self.scope.is_execution_terminating() {
            anyhow::bail!("Execution terminated");
        }
        let err: anyhow::Result<_> = try {
            let (message, frame_data, custom_data) =
                extract_source_mapped_error(self.scope, exception)?;
            JsError::from_frames(message, frame_data, custom_data, |s| {
                let context_state = self.context_state()?;
                let Some(source_map) = context_state.module_map.lookup_source_map(s) else {
                    return Ok(None);
                };
                Ok(source_map_from_slice(source_map.as_bytes()))
            })
        };
        let err = match err {
            Ok(e) => e,
            Err(e) => {
                let message = v8::Exception::create_message(self.scope, exception);
                let message = message.get(self.scope);
                let message = to_rust_string(self.scope, &message)?;
                metrics::log_source_map_failure(&message, &e);
                JsError::from_message(message)
            },
        };
        Ok(err)
    }
}
