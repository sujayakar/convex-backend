use deno_core::v8::{
    self,
    callback_scope,
};

use super::snapshot::{EntrypointSnapshot, GroupSnapshot};
use crate::strings;

pub struct Thread {
    pub isolate: v8::OwnedIsolate,
}

impl Thread {
    pub fn new() -> Self {
        let mut isolate =
            crate::udf_runtime::create_isolate_with_udf_runtime(v8::CreateParams::default());

        Self::configure_isolate(&mut isolate);
        Self { isolate }
    }

    /// Create a new thread from an entrypoint snapshot. The isolate's default
    /// context contains pre-compiled+evaluated modules.
    pub fn new_from_snapshot(snapshot: &EntrypointSnapshot) -> Self {
        let mut isolate = super::snapshot::create_isolate_from_snapshot(snapshot);

        Self::configure_isolate(&mut isolate);
        Self { isolate }
    }

    /// Create a new thread from a group snapshot.
    pub fn new_from_group_snapshot(snapshot: &GroupSnapshot) -> Self {
        let mut isolate = super::snapshot::create_isolate_from_group_snapshot(snapshot);

        Self::configure_isolate(&mut isolate);
        Self { isolate }
    }

    fn configure_isolate(isolate: &mut v8::OwnedIsolate) {
        isolate.set_capture_stack_trace_for_uncaught_exceptions(true, 10);
        isolate.set_host_initialize_import_meta_object_callback(Self::import_meta_callback);
        isolate.set_allow_atomics_wait(false);
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
    }

    extern "C" fn import_meta_callback(
        context: v8::Local<v8::Context>,
        _module: v8::Local<v8::Module>,
        _meta: v8::Local<v8::Object>,
    ) {
        callback_scope!(unsafe let scope, context);
        let message = strings::import_meta_unsupported
            .create(scope)
            .expect("Failed to create exception string");
        let exception = v8::Exception::type_error(scope, message);
        scope.throw_exception(exception);
    }
}
