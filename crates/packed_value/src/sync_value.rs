//! PackedSyncValue - A zero-copy value type for the sync protocol
//!
//! This replaces JsonPackedValue with a FlexBuffer-based representation
//! that can be passed through the sync worker without copying.

use std::sync::Arc;

use bytes::Bytes;
use serde::{
    Deserialize,
    Serialize,
    Serializer,
};
use serde_json::Value as JsonValue;
use value::{
    heap_size::HeapSize,
    ConvexValue,
};

use crate::{
    ByteBuffer,
    PackedValue,
};

/// A packed value optimized for zero-copy transfer through the sync protocol.
///
/// Internally uses FlexBuffer format for efficient storage and lazy access.
/// Can be serialized to JSON for WebSocket compatibility.
#[derive(Clone, Debug)]
pub struct PackedSyncValue {
    packed: PackedValue<ByteBuffer>,
    /// Cached JSON string for WebSocket serialization.
    /// Only populated on first JSON serialization.
    json_cache: Option<Arc<str>>,
}

impl PackedSyncValue {
    /// Create a new PackedSyncValue from a ConvexValue
    pub fn pack(value: &ConvexValue) -> Self {
        Self {
            packed: PackedValue::pack(value),
            json_cache: None,
        }
    }

    /// Create from an existing PackedValue (zero-copy if possible)
    pub fn from_packed(packed: PackedValue<ByteBuffer>) -> Self {
        Self {
            packed,
            json_cache: None,
        }
    }

    /// Create from raw FlexBuffer bytes
    pub fn from_bytes(bytes: Bytes) -> anyhow::Result<Self> {
        let packed = PackedValue::new(ByteBuffer::from(bytes));
        // Validate by opening
        let _ = packed.as_ref().open()?;
        Ok(Self {
            packed,
            json_cache: None,
        })
    }

    /// Get the underlying bytes (zero-copy)
    pub fn as_bytes(&self) -> &[u8] {
        self.packed.as_slice()
    }

    /// Get the size in bytes
    pub fn size(&self) -> usize {
        self.packed.size()
    }

    /// Unpack to a ConvexValue (requires allocation)
    pub fn unpack(&self) -> anyhow::Result<ConvexValue> {
        ConvexValue::try_from(self.packed.clone())
    }

    /// Get or create JSON representation (for WebSocket compatibility)
    pub fn to_json(&self) -> anyhow::Result<JsonValue> {
        let value = self.unpack()?;
        Ok(value.into())
    }

    /// Get the underlying PackedValue for direct access
    pub fn packed(&self) -> &PackedValue<ByteBuffer> {
        &self.packed
    }

    /// Consume and return the underlying PackedValue
    pub fn into_packed(self) -> PackedValue<ByteBuffer> {
        self.packed
    }

    /// Get access to the opened value for lazy property access
    pub fn open(&self) -> anyhow::Result<crate::OpenedValue<&[u8]>> {
        self.packed.as_ref().open()
    }
}

impl HeapSize for PackedSyncValue {
    fn heap_size(&self) -> usize {
        self.packed.heap_size() + self.json_cache.as_ref().map_or(0, |s| s.len())
    }
}

impl PartialEq for PackedSyncValue {
    fn eq(&self, other: &Self) -> bool {
        // Compare the underlying bytes
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for PackedSyncValue {}

impl std::hash::Hash for PackedSyncValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

// Serialize to JSON for WebSocket compatibility
impl Serialize for PackedSyncValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Unpack and serialize as JSON
        let value = self.unpack().map_err(serde::ser::Error::custom)?;
        let json_value: JsonValue = value.into();
        json_value.serialize(serializer)
    }
}

// Deserialize from JSON
impl<'de> Deserialize<'de> for PackedSyncValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let json_value = JsonValue::deserialize(deserializer)?;
        let value = ConvexValue::try_from(json_value).map_err(serde::de::Error::custom)?;
        Ok(Self::pack(&value))
    }
}

impl From<ConvexValue> for PackedSyncValue {
    fn from(value: ConvexValue) -> Self {
        Self::pack(&value)
    }
}

impl TryFrom<PackedSyncValue> for ConvexValue {
    type Error = anyhow::Error;

    fn try_from(value: PackedSyncValue) -> anyhow::Result<Self> {
        value.unpack()
    }
}

impl PackedSyncValue {
    /// Convert from a JsonPackedValue (for gradual migration)
    pub fn from_json_packed(json_packed: &value::JsonPackedValue) -> anyhow::Result<Self> {
        let value = json_packed.unpack()?;
        Ok(Self::pack(&value))
    }
}

#[cfg(any(test, feature = "testing"))]
impl proptest::arbitrary::Arbitrary for PackedSyncValue {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<PackedSyncValue>;

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::*;
        any::<ConvexValue>()
            .prop_map(|v| PackedSyncValue::pack(&v))
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_unpack_roundtrip() {
        let value = ConvexValue::from(42i64);
        let packed = PackedSyncValue::pack(&value);
        let unpacked = packed.unpack().unwrap();
        assert_eq!(value, unpacked);
    }

    #[test]
    fn test_clone_is_cheap() {
        let value = ConvexValue::try_from("hello world".to_string()).unwrap();
        let packed = PackedSyncValue::pack(&value);
        let cloned = packed.clone();
        // Both should point to the same underlying buffer
        assert_eq!(packed.as_bytes(), cloned.as_bytes());
    }

    #[test]
    fn test_json_serialization() {
        let value = ConvexValue::from(42i64);
        let packed = PackedSyncValue::pack(&value);
        let json = serde_json::to_string(&packed).unwrap();
        // Convex values serialize with type markers for non-JSON-native types
        // i64 becomes {"$integer": "base64-encoded-value"}
        let deserialized: PackedSyncValue = serde_json::from_str(&json).unwrap();
        assert_eq!(packed, deserialized);
    }
}
