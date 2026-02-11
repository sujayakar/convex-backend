use std::{
    collections::{
        BTreeMap,
        BTreeSet,
    },
    path::Path,
    time::{
        Duration,
        Instant,
    },
};

use common::{
    bootstrap_model::index::vector_index::VectorIndexSpec,
    document::ResolvedDocument,
    persistence::DocumentStream,
    query::search_value_to_bytes,
    types::WriteTimestamp,
};
use errors::ErrorMetadata;
use futures::TryStreamExt;
use pb::searchlight as proto;
use tempfile::TempDir;
use value::{
    ConvexValue,
    FieldPath,
};

use super::segment::{
    snapshot_segment,
    ExternalId,
    PreviousSegmentsHack,
    SpannSegment,
    VectorDiskSegmentValues,
    VectorDocument,
};
use crate::{
    id_tracker::VectorMemoryIdTracker,
    query::{
        CompiledVectorFilter,
        CompiledVectorSearch,
        InternalVectorSearch,
        VectorSearchExpression,
        VectorSearchQueryResult,
    },
    IndexedVector,
    DEFAULT_VECTOR_LIMIT,
    MAX_FILTER_LENGTH,
    MAX_VECTOR_RESULTS,
    VECTOR_ELEMENT_SIZE,
};

#[derive(Clone, Debug)]
pub struct VectorSchema {
    dimension: usize,
    vector_field: FieldPath,
    filter_fields: BTreeSet<FieldPath>,
}

impl VectorSchema {
    pub fn new(index_config: &VectorIndexSpec) -> Self {
        Self {
            dimension: u32::from(index_config.dimensions) as usize,
            vector_field: index_config.vector_field.clone(),
            filter_fields: index_config.filter_fields.clone(),
        }
    }

    pub fn index(&self, document: &ResolvedDocument) -> Option<VectorDocument> {
        let object = document.value();
        let Some(ConvexValue::Array(array)) = object.get_path(&self.vector_field) else {
            return None;
        };
        let mut vector = Vec::with_capacity(self.dimension);
        if array.len() != self.dimension {
            tracing::debug!(
                "Ignoring mismatched vector length, expected: {}, actual: {}",
                self.dimension,
                array.len(),
            );
            return None;
        }
        for value in array.iter() {
            let ConvexValue::Float64(f) = value else {
                return None;
            };
            vector.push(*f as f32);
        }
        let vector = IndexedVector::try_from(vector).ok()?;
        let document = VectorDocument {
            internal_id: document.internal_id(),
            vector,
            filter_fields: self
                .filter_fields
                .iter()
                .map(|f| (f.clone(), search_value_to_bytes(object.get_path(f))))
                .collect(),
        };
        Some(document)
    }

    pub fn estimate_vector_size(&self) -> usize {
        self.dimension * VECTOR_ELEMENT_SIZE
    }

    pub fn compile(&self, query: InternalVectorSearch) -> anyhow::Result<CompiledVectorSearch> {
        let timer = crate::metrics::compile_timer();

        let index_name = query.printable_index_name()?;
        let query_vector = IndexedVector::try_from(query.vector)?;
        let query_limit = query.limit.unwrap_or(DEFAULT_VECTOR_LIMIT);
        anyhow::ensure!(
            query_limit as usize <= MAX_VECTOR_RESULTS,
            ErrorMetadata::bad_request(
                "VectorLimitTooLargeError",
                format!(
                    "Vector queries can fetch at most {} results, requested {}.",
                    MAX_VECTOR_RESULTS, query_limit as usize,
                )
            )
        );
        let mut filter_conditions = std::collections::BTreeMap::new();
        let mut filter_length = 0;

        for expresion in query.expressions {
            match expresion {
                VectorSearchExpression::Eq(field_path, value) => {
                    if !self.filter_fields.contains(&field_path) {
                        anyhow::bail!(crate::incorrect_vector_filter_field_error(
                            &index_name,
                            &field_path
                        ))
                    }
                    let value_bytes = search_value_to_bytes(value.as_ref());
                    if filter_conditions.contains_key(&field_path) {
                        anyhow::bail!("Found multiple filters for the same field?")
                    }
                    filter_conditions.insert(field_path, CompiledVectorFilter::Eq(value_bytes));
                    filter_length += 1;
                },
                VectorSearchExpression::In(field_path, values) => {
                    if !self.filter_fields.contains(&field_path) {
                        anyhow::bail!(crate::incorrect_vector_filter_field_error(
                            &index_name,
                            &field_path
                        ))
                    }
                    let values_bytes: Vec<_> = values
                        .into_iter()
                        .map(|v| search_value_to_bytes(v.as_ref()))
                        .collect();
                    if filter_conditions.contains_key(&field_path) {
                        anyhow::bail!("Found multiple filters for the same field?")
                    }
                    filter_length += values_bytes.len();
                    filter_conditions.insert(field_path, CompiledVectorFilter::In(values_bytes));
                },
            }
        }
        anyhow::ensure!(
            filter_length <= MAX_FILTER_LENGTH,
            ErrorMetadata::bad_request(
                "TooManyElementsInVectorQueryError",
                format!(
                    "Vector query against {index_name} has too many conditions. Max: \
                     {MAX_FILTER_LENGTH} Actual: {filter_length}"
                )
            )
        );
        anyhow::ensure!(
            query_vector.len() == self.dimension,
            crate::vector_dimensions_mismatch_error(
                query_vector.len() as u32,
                self.dimension as u32
            )
        );
        let result = CompiledVectorSearch {
            vector: query_vector,
            limit: query_limit,
            filter_conditions,
        };
        crate::metrics::log_compiled_query(&result);
        timer.finish();
        Ok(result)
    }

    pub fn search(
        &self,
        segment: &SpannSegment,
        query: CompiledVectorSearch,
        overfetch_delta: u32,
        slow_vector_query_threshold_millis: u64,
        _require_exact: bool,
    ) -> anyhow::Result<Vec<VectorSearchQueryResult>> {
        let start = Instant::now();
        let segment_results = segment.search(
            &query.vector,
            &query.filter_conditions,
            (query.limit + overfetch_delta) as usize,
        );
        let duration = start.elapsed();
        if duration > Duration::from_millis(slow_vector_query_threshold_millis) {
            tracing::warn!(
                "Slow vector query, duration: {}ms, schema: {:?}, overfetch_delta: {}",
                duration.as_millis(),
                self,
                overfetch_delta,
            );
        }

        let mut results = Vec::with_capacity(segment_results.len());
        for (point_index, score, ts) in segment_results {
            let internal_id = segment.internal_id_for_vector(point_index).ok_or_else(|| {
                anyhow::anyhow!("Missing internal id for point index {point_index}")
            })?;
            results.push(VectorSearchQueryResult {
                score,
                id: internal_id,
                ts: WriteTimestamp::Committed(ts.try_into()?),
            });
        }
        Ok(results)
    }

    pub async fn build_disk_index<T: PreviousSegmentsHack>(
        &self,
        index_path: &Path,
        revision_stream: DocumentStream<'_>,
        _hnsw_threshold_bytes: usize,
        previous_segments: &mut T,
    ) -> anyhow::Result<Option<VectorDiskSegmentValues>> {
        let mut latest_documents =
            BTreeMap::<_, (crate::IndexedVector, BTreeMap<FieldPath, Vec<u8>>, u64)>::new();
        futures::pin_mut!(revision_stream);
        while let Some(entry) = revision_stream.try_next().await? {
            let external_id = ExternalId::try_from(&entry.id)?;
            if let Some(document) = entry.value {
                let Some(vector_document) = self.index(&document) else {
                    tracing::trace!("Skipping an invalid doc: {:?}", document);
                    previous_segments.maybe_delete_vector(external_id)?;
                    continue;
                };
                latest_documents.insert(
                    vector_document.internal_id,
                    (
                        vector_document.vector,
                        vector_document.filter_fields,
                        u64::from(entry.ts),
                    ),
                );
            } else {
                latest_documents.remove(&entry.id.internal_id());
            }
            // Updates or deletes of documents need to clear out old versions of those docs
            // in previous segments, and we conservatively do this for all stream entries.
            previous_segments.maybe_delete_vector(external_id)?;
        }

        if latest_documents.is_empty() {
            tracing::debug!("Skipping an empty vector index for {index_path:?}");
            return Ok(None);
        }

        std::fs::create_dir_all(index_path)?;
        let tmpdir = TempDir::new()?;
        let mut id_tracker = VectorMemoryIdTracker::new();
        let documents: Vec<_> = latest_documents
            .into_iter()
            .enumerate()
            .map(
                |(offset, (internal_id, (vector, filter_fields, timestamp)))| -> anyhow::Result<_> {
                    id_tracker.set_link(internal_id, offset as u32)?;
                    Ok((internal_id, vector, filter_fields, timestamp))
                },
            )
            .try_collect()?;

        let segment = SpannSegment::build(documents, self.dimension);
        let num_vectors = segment.num_vectors() as u32;
        let paths = snapshot_segment(&mut id_tracker, &segment, tmpdir.path(), index_path)?;

        Ok(Some(VectorDiskSegmentValues {
            paths,
            num_vectors,
            num_deleted: 0,
        }))
    }
}

impl From<VectorSchema> for proto::VectorIndexConfig {
    fn from(value: VectorSchema) -> Self {
        proto::VectorIndexConfig {
            dimension: value.dimension as u32,
            vector_field_path: Some(value.vector_field.into()),
            filter_fields: value.filter_fields.into_iter().map(|f| f.into()).collect(),
        }
    }
}

impl TryFrom<proto::VectorIndexConfig> for VectorSchema {
    type Error = anyhow::Error;

    fn try_from(value: proto::VectorIndexConfig) -> Result<Self, Self::Error> {
        let vector_field = value
            .vector_field_path
            .ok_or_else(|| anyhow::anyhow!("Missing vector field path in VectorIndexConfigProto"))?
            .try_into()?;
        let filter_fields = value
            .filter_fields
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<_, _>>()?;
        Ok(VectorSchema {
            dimension: value.dimension as usize,
            vector_field,
            filter_fields,
        })
    }
}

/// Cosine similarity between two vectors (raw, will be normalized internally).
/// Public test helper - for pre-normalized vectors use
/// `spann::cosine::cosine_similarity`.
#[cfg(any(test, feature = "testing"))]
pub fn cosine_similarity(v1: &[f32], v2: &[f32]) -> f32 {
    let v1 = super::cosine::preprocess(v1.to_vec());
    let v2 = super::cosine::preprocess(v2.to_vec());
    super::cosine::cosine_similarity(&v1, &v2)
}
