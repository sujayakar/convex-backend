use anyhow::Context as _;
use deno_core::{
    serde_v8,
    v8,
};
use serde::{
    Deserialize,
    Serialize,
};

use packed_value::{
    ByteBuffer,
    OpenedValue,
    PackedValue,
};

pub type PackedValueHandle = u64;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PackedValueKind {
    Object,
    Array,
}

impl PackedValueKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Array => "array",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackedPathSegment {
    Field(String),
    Index(usize),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackedProxyMeta {
    pub handle: PackedValueHandle,
    pub kind: PackedValueKind,
    pub path: Vec<PackedPathSegment>,
}

pub fn extract_packed_meta<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> anyhow::Result<Option<PackedProxyMeta>> {
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return Ok(None);
    };
    let key = v8::String::new(scope, "convex.packed")
        .context("failed to create packed symbol key")?;
    let symbol = v8::Symbol::for_key(scope, key);
    let Some(meta_value) = object.get(scope, symbol.into()) else {
        return Ok(None);
    };
    if meta_value.is_undefined() || meta_value.is_null() {
        return Ok(None);
    }
    let meta: PackedProxyMeta = serde_v8::from_v8(scope, meta_value)?;
    Ok(Some(meta))
}

pub fn open_at_path(
    packed: &PackedValue<ByteBuffer>,
    path: &[PackedPathSegment],
) -> anyhow::Result<Option<OpenedValue<ByteBuffer>>> {
    let mut current = packed.clone().open()?;
    for segment in path {
        match segment {
            PackedPathSegment::Field(field) => match current {
                OpenedValue::Object(object) => match object.get(field)? {
                    Some(next) => current = next,
                    None => return Ok(None),
                },
                _ => return Ok(None),
            },
            PackedPathSegment::Index(index) => match current {
                OpenedValue::Array(array) => {
                    if *index >= array.len() {
                        return Ok(None);
                    }
                    current = array.index(*index)?;
                },
                _ => return Ok(None),
            },
        }
    }
    Ok(Some(current))
}

