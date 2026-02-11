#![feature(iterator_try_collect)]
#![feature(int_roundings)]
#![feature(type_alias_impl_trait)]
#![feature(coroutines)]
#![feature(coroutine_trait)]
#![feature(try_blocks)]
#![feature(impl_trait_in_assoc_type)]

use std::ops::Deref;

use common::{
    bootstrap_model::index::vector_index::MAX_VECTOR_DIMENSIONS,
    types::IndexName,
};
use errors::ErrorMetadata;
use value::FieldPath;

pub mod id_tracker;
mod memory_index;
pub mod metrics;
pub mod qdrant_segments;
mod query;
mod searcher;
pub mod spann;
mod vector_index_manager;

#[cfg(test)]
mod integration_tests;

#[cfg(any(test, feature = "testing"))]
pub use self::spann::cosine_similarity;
// Backward-compat aliases for code that imports the old names.
pub use self::spann::VectorSchema as QdrantSchema;
pub use self::{
    memory_index::MemoryVectorIndex,
    metrics::{
        vector_index_type_label,
        VectorIndexType,
        VECTOR_INDEX_TYPE_LABEL,
    },
    query::{
        CompiledVectorSearch,
        InternalVectorSearch,
        PublicVectorSearchQueryResult,
        VectorSearch,
        VectorSearchExpression,
        VectorSearchJson,
        VectorSearchQueryResult,
        VectorSearchRequest,
    },
    searcher::VectorSearcher,
    spann::{
        load_disk_segment,
        merge_disk_segments,
        restore_segment_from_tar,
        segment::{
            ExternalId as QdrantExternalId,
            PreviousSegmentsHack as PreviousVectorSegmentsHack,
        },
        NormalizedVectorDocument,
        SpannSegment,
        UntarredVectorDiskSegmentPaths,
        VectorDiskSegmentPaths,
        VectorDiskSegmentValues,
        VectorDocument,
        VectorDocument as QdrantDocument,
        VectorSchema,
    },
    vector_index_manager::{
        IndexState,
        VectorIndexManager,
    },
};

pub const MAX_VECTOR_RESULTS: usize = 256;
pub const DEFAULT_VECTOR_LIMIT: u32 = 10;
pub const MAX_FILTER_LENGTH: usize = 64;
pub const VECTOR_ELEMENT_SIZE: usize = std::mem::size_of::<f32>();

#[derive(Clone, Debug)]
pub struct IndexedVector(Vec<f32>);

impl Deref for IndexedVector {
    type Target = [f32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<Vec<f32>> for IndexedVector {
    type Error = anyhow::Error;

    fn try_from(value: Vec<f32>) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.len() <= MAX_VECTOR_DIMENSIONS as usize,
            vector_dimensions_mismatch_error(value.len() as u32, MAX_VECTOR_DIMENSIONS)
        );
        Ok(IndexedVector(value))
    }
}

impl From<IndexedVector> for Vec<f32> {
    fn from(value: IndexedVector) -> Self {
        value.0
    }
}

fn vector_dimensions_mismatch_error(dimensions: u32, expected_dimensions: u32) -> ErrorMetadata {
    ErrorMetadata::bad_request(
        "VectorDimensionsMismatch",
        format!("Expected a vector with dimensions {expected_dimensions}, received {dimensions}."),
    )
}

fn incorrect_vector_filter_field_error(
    index_name: &IndexName,
    field_path: &FieldPath,
) -> ErrorMetadata {
    ErrorMetadata::bad_request(
        "IncorrectVectorFilterField",
        format!(
            "Vector query against {index_name} contains a filter on {field_path:?} but that field \
             isn't indexed for filtering in `filterFields`."
        ),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VectorIndexWriteSize(pub u64);
