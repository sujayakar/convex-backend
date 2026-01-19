use anyhow::Context as _;
use serde::Serialize;
use serde_json::Value as JsonValue;
use value::ConvexValue;

use crate::{
    packed_values::{
        PackedPathSegment,
        PackedValueHandle,
        PackedValueKind,
    },
};

use super::OpProvider;
use packed_value::{
    ByteBuffer,
    OpenedValue,
    PackedValue,
};

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum PackedReadResult {
    Missing,
    Value {
        value: JsonValue,
    },
    Packed {
        path: Vec<PackedPathSegment>,
        value_kind: PackedValueKind,
    },
}

fn open_at_path(
    packed: &PackedValue<ByteBuffer>,
    path: &[PackedPathSegment],
) -> anyhow::Result<Option<OpenedValue<ByteBuffer>>> {
    let mut current = packed.clone().open()?;
    for segment in path {
        match segment {
            PackedPathSegment::Field(field) => match current {
                OpenedValue::Object(object) => {
                    match object.get(field)? {
                        Some(next) => current = next,
                        None => return Ok(None),
                    }
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

#[convex_macro::v8_op]
pub fn op_packed_value_read<'b, P: OpProvider<'b>>(
    provider: &mut P,
    handle: PackedValueHandle,
    mut path: Vec<PackedPathSegment>,
    key: PackedPathSegment,
) -> anyhow::Result<PackedReadResult> {
    let packed = provider
        .get_packed_value(handle)?
        .with_context(|| format!("Packed value {handle} missing"))?;
    path.push(key);
    let Some(value) = open_at_path(&packed, &path)? else {
        return Ok(PackedReadResult::Missing);
    };
    match value {
        OpenedValue::Object(_) => Ok(PackedReadResult::Packed {
            path,
            value_kind: PackedValueKind::Object,
        }),
        OpenedValue::Array(_) => Ok(PackedReadResult::Packed {
            path,
            value_kind: PackedValueKind::Array,
        }),
        primitive => {
            let value = ConvexValue::try_from(primitive)?;
            let json = serde_json::to_value(value)?;
            Ok(PackedReadResult::Value { value: json })
        },
    }
}

#[convex_macro::v8_op]
pub fn op_packed_value_keys<'b, P: OpProvider<'b>>(
    provider: &mut P,
    handle: PackedValueHandle,
    path: Vec<PackedPathSegment>,
) -> anyhow::Result<Vec<String>> {
    let packed = provider
        .get_packed_value(handle)?
        .with_context(|| format!("Packed value {handle} missing"))?;
    let value = open_at_path(&packed, &path)?
        .context("Packed value path not found for keys")?;
    match value {
        OpenedValue::Object(object) => object
            .iter()
            .map(|entry| {
                let (key, _) = entry?;
                Ok(key.to_string())
            })
            .collect(),
        OpenedValue::Array(array) => Ok((0..array.len()).map(|i| i.to_string()).collect()),
        _ => anyhow::bail!("Packed value is not an object or array"),
    }
}

#[convex_macro::v8_op]
pub fn op_packed_value_length<'b, P: OpProvider<'b>>(
    provider: &mut P,
    handle: PackedValueHandle,
    path: Vec<PackedPathSegment>,
) -> anyhow::Result<usize> {
    let packed = provider
        .get_packed_value(handle)?
        .with_context(|| format!("Packed value {handle} missing"))?;
    let value = open_at_path(&packed, &path)?
        .context("Packed value path not found for length")?;
    match value {
        OpenedValue::Array(array) => Ok(array.len()),
        _ => anyhow::bail!("Packed value is not an array"),
    }
}

#[convex_macro::v8_op]
pub fn op_packed_value_has<'b, P: OpProvider<'b>>(
    provider: &mut P,
    handle: PackedValueHandle,
    path: Vec<PackedPathSegment>,
    key: PackedPathSegment,
) -> anyhow::Result<bool> {
    let packed = provider
        .get_packed_value(handle)?
        .with_context(|| format!("Packed value {handle} missing"))?;
    let value = open_at_path(&packed, &path)?
        .context("Packed value path not found for has")?;
    match (value, key) {
        (OpenedValue::Object(object), PackedPathSegment::Field(field)) => {
            Ok(object.get(&field)?.is_some())
        },
        (OpenedValue::Array(array), PackedPathSegment::Index(index)) => Ok(index < array.len()),
        _ => Ok(false),
    }
}
