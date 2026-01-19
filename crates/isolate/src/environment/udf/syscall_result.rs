use anyhow::Context as _;
use deno_core::v8;
use serde_json::Value as JsonValue;

use crate::{
    convert_v8::ToV8,
    packed_values::{
        PackedValueHandle,
        PackedValueKind,
    },
    strings,
};

#[derive(Debug)]
pub enum SyscallResult {
    Json(JsonValue),
    Value(SyscallValue),
}

#[derive(Debug)]
pub enum SyscallValue {
    PackedDoc {
        handle: PackedValueHandle,
    },
    QueryStreamNext {
        value: Option<PackedValueHandle>,
        done: bool,
    },
    QueryPage {
        page: Vec<PackedValueHandle>,
        is_done: bool,
        continue_cursor: String,
        split_cursor: Option<String>,
        page_status: Option<&'static str>,
    },
}

impl ToV8 for SyscallResult {
    fn to_v8<'s>(
        self,
        scope: &mut v8::PinScope<'s, '_>,
    ) -> anyhow::Result<v8::Local<'s, v8::Value>> {
        match self {
            SyscallResult::Json(value) => {
                let json = serde_json::to_string(&value)?;
                let value_v8 = v8::String::new(scope, &json)
                    .context("failed to create syscall json string")?;
                Ok(value_v8.into())
            },
            SyscallResult::Value(value) => value.to_v8(scope),
        }
    }
}

impl SyscallValue {
    fn to_v8<'s>(
        self,
        scope: &mut v8::PinScope<'s, '_>,
    ) -> anyhow::Result<v8::Local<'s, v8::Value>> {
        match self {
            SyscallValue::PackedDoc { handle } => {
                create_packed_proxy(scope, handle, PackedValueKind::Object)
            },
            SyscallValue::QueryStreamNext { value, done } => {
                let result = v8::Object::new(scope);
                let value_key = v8::String::new(scope, "value")
                    .context("failed to create value key")?;
                let value_v8 = match value {
                    Some(handle) => create_packed_proxy(scope, handle, PackedValueKind::Object)?,
                    None => v8::null(scope).into(),
                };
                result.set(scope, value_key.into(), value_v8);
                let done_key = v8::String::new(scope, "done")
                    .context("failed to create done key")?;
                result.set(scope, done_key.into(), v8::Boolean::new(scope, done).into());
                Ok(result.into())
            },
            SyscallValue::QueryPage {
                page,
                is_done,
                continue_cursor,
                split_cursor,
                page_status,
            } => {
                let result = v8::Object::new(scope);
                let page_key = v8::String::new(scope, "page")
                    .context("failed to create page key")?;
                let page_array = v8::Array::new(scope, page.len() as i32);
                for (idx, handle) in page.into_iter().enumerate() {
                    let proxy = create_packed_proxy(scope, handle, PackedValueKind::Object)?;
                    page_array.set_index(scope, idx as u32, proxy);
                }
                result.set(scope, page_key.into(), page_array.into());

                let is_done_key = v8::String::new(scope, "isDone")
                    .context("failed to create isDone key")?;
                result.set(scope, is_done_key.into(), v8::Boolean::new(scope, is_done).into());

                let continue_key = v8::String::new(scope, "continueCursor")
                    .context("failed to create continueCursor key")?;
                let continue_value =
                    v8::String::new(scope, &continue_cursor).context("failed to create cursor")?;
                result.set(scope, continue_key.into(), continue_value.into());

                let split_key = v8::String::new(scope, "splitCursor")
                    .context("failed to create splitCursor key")?;
                let split_value = match split_cursor {
                    Some(cursor) => v8::String::new(scope, &cursor)
                        .context("failed to create split cursor")?
                        .into(),
                    None => v8::null(scope).into(),
                };
                result.set(scope, split_key.into(), split_value);

                let status_key = v8::String::new(scope, "pageStatus")
                    .context("failed to create pageStatus key")?;
                let status_value = match page_status {
                    Some(status) => v8::String::new(scope, status)
                        .context("failed to create page status")?
                        .into(),
                    None => v8::null(scope).into(),
                };
                result.set(scope, status_key.into(), status_value);

                Ok(result.into())
            },
        }
    }
}

fn create_packed_proxy<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    handle: PackedValueHandle,
    kind: PackedValueKind,
) -> anyhow::Result<v8::Local<'s, v8::Value>> {
    let global = scope.get_current_context().global(scope);
    let internal_key = strings::__convexInternal.create(scope)?;
    let internal = global
        .get(scope, internal_key.into())
        .context("missing __convexInternal")?
        .to_object(scope)
        .context("invalid __convexInternal")?;
    let create_key = strings::createPackedValue.create(scope)?;
    let create_value = internal
        .get(scope, create_key.into())
        .context("missing createPackedValue")?;
    let create_fn: v8::Local<v8::Function> = create_value.try_into()?;

    let handle_value = v8::Number::new(scope, handle as f64);
    let kind_value = v8::String::new(scope, kind.as_str())
        .context("failed to create packed kind")?;
    let path_value = v8::Array::new(scope, 0);
    let result = create_fn
        .call(
            scope,
            internal.into(),
            &[handle_value.into(), kind_value.into(), path_value.into()],
        )
        .context("failed to create packed proxy")?;
    Ok(result)
}
