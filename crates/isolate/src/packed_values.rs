use serde::{
    Deserialize,
    Serialize,
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

