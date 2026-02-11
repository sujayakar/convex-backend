use std::{
    collections::BTreeMap,
    fs::{
        self,
        File,
    },
    io::{
        BufReader,
        BufWriter,
        Read as IoRead,
        Write as IoWrite,
    },
    mem,
    path::{
        Path,
        PathBuf,
    },
};

use common::{
    deleted_bitset::DeletedBitset,
    runtime::tokio_spawn_blocking,
};
use uuid::Uuid;
use value::{
    base64,
    FieldPath,
    InternalDocumentId,
    InternalId,
    ResolvedDocumentId,
};

use super::{
    clustering,
    cosine,
};
use crate::{
    id_tracker::{
        VectorMemoryIdTracker,
        VectorStaticIdTracker,
    },
    IndexedVector,
    VECTOR_ELEMENT_SIZE,
};

const UUID_TABLE_FILENAME: &str = "uuids.table";
const DELETED_BITSET_FILENAME: &str = "deleted.bitset";
const VECTORS_FILENAME: &str = "vectors.bin";
const VECTORS_F16_FILENAME: &str = "vectors_q.f16";
const PAYLOADS_FILENAME: &str = "payloads.bin";
const CENTROIDS_FILENAME: &str = "centroids.bin";
const POSTINGS_FILENAME: &str = "postings.bin";
const ORIGINALS_FILENAME: &str = "originals.bin";

/// Minimum number of vectors to trigger centroid-based partitioning.
/// Below this threshold, we use a flat (brute-force) scan.
const CLUSTERING_THRESHOLD: usize = 256;

/// Target number of vectors per posting (partition).
const TARGET_POSTING_SIZE: usize = 100;

/// Default number of postings to probe during search.
pub const DEFAULT_NPROBE: usize = 32;

/// Boundary replication factor: if a vector's distance to its 2nd-nearest
/// centroid is within (1 + BOUNDARY_EPSILON) * distance to nearest centroid,
/// replicate it into the 2nd-nearest posting for improved recall.
const BOUNDARY_EPSILON: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExternalId(pub Uuid);

impl TryFrom<InternalId> for ExternalId {
    type Error = anyhow::Error;

    fn try_from(value: InternalId) -> Result<Self, Self::Error> {
        Ok(Self(Uuid::from_bytes(value.0)))
    }
}

impl TryFrom<&InternalDocumentId> for ExternalId {
    type Error = anyhow::Error;

    fn try_from(value: &InternalDocumentId) -> Result<Self, Self::Error> {
        Ok(Self(Uuid::from_bytes(value.internal_id().0)))
    }
}

impl TryFrom<ResolvedDocumentId> for ExternalId {
    type Error = anyhow::Error;

    fn try_from(value: ResolvedDocumentId) -> Result<Self, Self::Error> {
        Ok(Self(Uuid::from_bytes(value.internal_id().0)))
    }
}

impl std::ops::Deref for ExternalId {
    type Target = Uuid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A circular dependency workaround for search / database / vector.
pub trait PreviousSegmentsHack {
    fn maybe_delete_vector(&mut self, external_id: ExternalId) -> anyhow::Result<()>;
}

#[derive(Clone, Debug)]
pub struct VectorDocument {
    pub internal_id: InternalId,
    pub vector: IndexedVector,
    pub filter_fields: BTreeMap<FieldPath, Vec<u8>>,
}

impl VectorDocument {
    pub fn estimate_size(&self) -> usize {
        self.vector.len() * VECTOR_ELEMENT_SIZE
    }
}

#[derive(Clone, Debug)]
pub struct NormalizedVectorDocument {
    pub internal_id: InternalId,
    pub vector: Vec<f32>,
    pub filter_fields: BTreeMap<FieldPath, Vec<u8>>,
}

impl From<VectorDocument> for NormalizedVectorDocument {
    fn from(value: VectorDocument) -> Self {
        let vector = Vec::from(value.vector);
        let vector = cosine::preprocess(vector);
        Self {
            internal_id: value.internal_id,
            vector,
            filter_fields: value.filter_fields,
        }
    }
}

impl NormalizedVectorDocument {
    pub fn size(&self) -> usize {
        let mut size = 0;
        size += self.vector.len() * mem::size_of::<f32>();
        size += self.filter_fields.len() * mem::size_of::<(FieldPath, Vec<u8>)>();
        for (field_path, maybe_value) in &self.filter_fields {
            size += field_path.fields().iter().map(|f| f.len()).sum::<usize>();
            size += maybe_value.len();
        }
        size
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Hash)]
pub struct VectorDiskSegmentPaths {
    pub segment: PathBuf,
    pub uuids: PathBuf,
    pub deleted_bitset: PathBuf,
}

#[derive(PartialEq, Eq, Debug, Clone, Hash)]
pub struct UntarredVectorDiskSegmentPaths {
    pub segment_dir: PathBuf,
    pub uuids: PathBuf,
    pub deleted_bitset: PathBuf,
}

impl UntarredVectorDiskSegmentPaths {
    pub fn from(untarred_segment: PathBuf, paths: VectorDiskSegmentPaths) -> Self {
        Self::new(untarred_segment, paths.uuids, paths.deleted_bitset)
    }

    pub fn new(untarred_segment: PathBuf, uuids: PathBuf, deleted_bitset: PathBuf) -> Self {
        Self {
            segment_dir: untarred_segment,
            uuids,
            deleted_bitset,
        }
    }
}

#[derive(Debug)]
pub struct VectorDiskSegmentValues {
    pub paths: VectorDiskSegmentPaths,
    pub num_vectors: u32,
    pub num_deleted: u32,
}

/// Storage for contiguous vector data. Can be either a heap-allocated
/// `Vec<f32>` (for freshly built segments) or a zero-copy memory-mapped view
/// into a file (for segments loaded from disk).
pub(crate) enum VectorData {
    /// Heap-allocated vectors (used during build and for small segments).
    Owned(Vec<f32>),
    /// Memory-mapped vectors file. The `Mmap` covers the entire vectors.bin
    /// including the 8-byte header; `data_offset` is 8 (past the header) and
    /// `num_floats` is the number of f32 elements in the data region.
    Mmap {
        mmap: memmap2::Mmap,
        data_offset: usize,
        num_floats: usize,
    },
}

impl VectorData {
    /// Get a `&[f32]` view over the vector data regardless of backing storage.
    fn as_f32_slice(&self) -> &[f32] {
        match self {
            VectorData::Owned(v) => v.as_slice(),
            VectorData::Mmap {
                mmap,
                data_offset,
                num_floats,
            } => {
                let byte_slice = &mmap[*data_offset..][..*num_floats * 4];
                // Safety: vectors.bin is written with native-endian f32 bytes
                // and the data region starts at offset 8 (aligned to 4).
                unsafe {
                    std::slice::from_raw_parts(byte_slice.as_ptr() as *const f32, *num_floats)
                }
            },
        }
    }
}

/// A loaded SPANN segment ready for search.
///
/// For small segments (< CLUSTERING_THRESHOLD vectors), this uses a flat
/// brute-force scan. For larger segments, it uses centroid-based partitioning
/// where vectors are grouped into postings and search routes through the
/// nearest centroids first.
pub struct SpannSegment {
    vectors: VectorData,
    payloads: Vec<SegmentPayload>,
    dimension: usize,
    num_original_vectors: usize,
    num_total_vectors: usize,

    pub(crate) centroids: VectorData,
    pub num_centroids: usize,
    vectors_f16_mmap: Option<memmap2::Mmap>,
    pub(crate) posting_assignments: Vec<u32>,
    original_indices: Vec<u32>,
    pub(crate) deleted_bitset: Option<DeletedBitset>,
}

#[derive(Clone, Debug)]
struct SegmentPayload {
    internal_id: InternalId,
    filter_fields: BTreeMap<FieldPath, Vec<u8>>,
    timestamp: u64,
}

impl SpannSegment {
    /// Search the segment with a query vector, returning scored results.
    ///
    /// For flat segments (no centroids), this is a brute-force scan.
    /// For partitioned segments, this probes the nearest `nprobe` centroids
    /// and scans only those postings.
    pub fn search(
        &self,
        query: &[f32],
        filter_conditions: &BTreeMap<FieldPath, crate::query::CompiledVectorFilter>,
        limit: usize,
    ) -> Vec<(u32, f32, u64)> {
        let query_normalized = cosine::preprocess(query.to_vec());

        if self.num_centroids == 0 {
            // Flat scan
            self.flat_search(&query_normalized, filter_conditions, limit)
        } else {
            // Centroid-routed search
            self.centroid_search(&query_normalized, filter_conditions, limit, DEFAULT_NPROBE)
        }
    }

    /// Search with a custom nprobe value (for tuning/testing).
    pub fn search_with_nprobe(
        &self,
        query: &[f32],
        filter_conditions: &BTreeMap<FieldPath, crate::query::CompiledVectorFilter>,
        limit: usize,
        nprobe: usize,
    ) -> Vec<(u32, f32, u64)> {
        let query_normalized = cosine::preprocess(query.to_vec());
        if self.num_centroids == 0 {
            self.flat_search(&query_normalized, filter_conditions, limit)
        } else {
            self.centroid_search(&query_normalized, filter_conditions, limit, nprobe)
        }
    }

    fn flat_search(
        &self,
        query_normalized: &[f32],
        filter_conditions: &BTreeMap<FieldPath, crate::query::CompiledVectorFilter>,
        limit: usize,
    ) -> Vec<(u32, f32, u64)> {
        let mut results: Vec<(u32, f32, u64)> = Vec::new();
        let vectors = self.vectors.as_f32_slice();

        for i in 0..self.num_total_vectors {
            if self.is_deleted(i as u32) {
                continue;
            }

            let payload = &self.payloads[i];
            if !Self::matches_filter(&payload.filter_fields, filter_conditions) {
                continue;
            }

            let score = self.vector_score(i, query_normalized, vectors);
            results.push((i as u32, score, payload.timestamp));
        }

        results.sort_by(|a, b| b.1.total_cmp(&a.1));
        results.truncate(limit);
        results
    }

    fn centroid_search(
        &self,
        query_normalized: &[f32],
        filter_conditions: &BTreeMap<FieldPath, crate::query::CompiledVectorFilter>,
        limit: usize,
        nprobe: usize,
    ) -> Vec<(u32, f32, u64)> {
        let nprobe = nprobe.min(self.num_centroids);
        let centroids = self.centroids.as_f32_slice();
        let mut centroid_scores: Vec<(usize, f32)> = (0..self.num_centroids)
            .map(|c| {
                let c_start = c * self.dimension;
                let c_end = c_start + self.dimension;
                let sim = cosine::cosine_similarity(query_normalized, &centroids[c_start..c_end]);
                (c, sim)
            })
            .collect();
        centroid_scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        centroid_scores.truncate(nprobe);

        let selected_centroids: std::collections::HashSet<u32> =
            centroid_scores.iter().map(|(c, _)| *c as u32).collect();

        // Scan vectors in selected postings, dedup by point index
        // (a vector may appear in multiple postings due to boundary replication)
        let mut seen = std::collections::HashSet::new();
        let mut results: Vec<(u32, f32, u64)> = Vec::new();
        let vectors = self.vectors.as_f32_slice();
        for i in 0..self.num_total_vectors {
            if !selected_centroids.contains(&self.posting_assignments[i]) {
                continue;
            }

            if self.is_deleted(i as u32) {
                continue;
            }

            if !seen.insert(self.original_indices[i]) {
                continue;
            }

            let payload = &self.payloads[i];
            if !Self::matches_filter(&payload.filter_fields, filter_conditions) {
                continue;
            }

            let score = self.vector_score(i, query_normalized, vectors);
            results.push((self.original_indices[i], score, payload.timestamp));
        }

        results.sort_by(|a, b| b.1.total_cmp(&a.1));
        results.truncate(limit);
        results
    }

    #[inline]
    fn vector_score(&self, i: usize, query_normalized: &[f32], vectors: &[f32]) -> f32 {
        if let Some(f16_mmap) = &self.vectors_f16_mmap {
            let f16_bytes_per_vector = self.dimension * 2;
            let f16_start = i * f16_bytes_per_vector;
            let f16_end = f16_start + f16_bytes_per_vector;
            let f16_slice = &f16_mmap[f16_start..f16_end];
            crate::spann::quantization::dot_product_f16(f16_slice, query_normalized, self.dimension)
        } else {
            let vec_start = i * self.dimension;
            let vec_end = vec_start + self.dimension;
            let vec_slice = &vectors[vec_start..vec_end];
            cosine::cosine_similarity(query_normalized, vec_slice)
        }
    }

    fn matches_filter(
        filter_fields: &BTreeMap<FieldPath, Vec<u8>>,
        filter_conditions: &BTreeMap<FieldPath, crate::query::CompiledVectorFilter>,
    ) -> bool {
        use crate::query::CompiledVectorFilter;
        if filter_conditions.is_empty() {
            return true;
        }
        for (field_path, condition) in filter_conditions {
            let Some(value) = filter_fields.get(field_path) else {
                return false;
            };
            let matches = match condition {
                CompiledVectorFilter::Eq(term) => term == value,
                CompiledVectorFilter::In(terms) => terms.iter().any(|t| t == value),
            };
            if matches {
                return true;
            }
        }
        false
    }

    /// Build a segment from a list of documents.
    ///
    /// For small document sets (< CLUSTERING_THRESHOLD), builds a flat segment.
    /// For larger sets, runs balanced k-means to partition vectors into
    /// postings and stores centroid vectors for routing.
    pub fn build(
        documents: Vec<(InternalId, IndexedVector, BTreeMap<FieldPath, Vec<u8>>, u64)>,
        dimension: usize,
    ) -> Self {
        let num_original_vectors = documents.len();
        let mut vectors = Vec::with_capacity(num_original_vectors * dimension);
        let mut payloads = Vec::with_capacity(num_original_vectors);

        for (id, vector, filter_fields, timestamp) in &documents {
            let normalized = cosine::preprocess(Vec::from(vector.clone()));
            vectors.extend_from_slice(&normalized);
            payloads.push(SegmentPayload {
                internal_id: *id,
                filter_fields: filter_fields.clone(),
                timestamp: *timestamp,
            });
        }

        if num_original_vectors < CLUSTERING_THRESHOLD {
            // Flat segment -- no centroids
            let original_indices = (0..num_original_vectors).map(|i| i as u32).collect();
            SpannSegment {
                vectors: VectorData::Owned(vectors),
                payloads,
                dimension,
                num_original_vectors,
                num_total_vectors: num_original_vectors,
                centroids: VectorData::Owned(Vec::new()),
                num_centroids: 0,
                vectors_f16_mmap: None,
                posting_assignments: Vec::new(),
                original_indices,
                deleted_bitset: None,
            }
        } else {
            // Run balanced k-means
            let k = (num_original_vectors / TARGET_POSTING_SIZE).max(2);
            let normalized_vecs: Vec<Vec<f32>> = (0..num_original_vectors)
                .map(|i| {
                    let start = i * dimension;
                    vectors[start..start + dimension].to_vec()
                })
                .collect();

            let clustering_result =
                clustering::balanced_kmeans(&normalized_vecs, dimension, k, 100.0);

            // Store centroids (normalize them for cosine search)
            let mut centroid_vecs: Vec<Vec<f32>> =
                Vec::with_capacity(clustering_result.centroids.len());
            let mut centroids_flat =
                Vec::with_capacity(clustering_result.centroids.len() * dimension);
            for c in &clustering_result.centroids {
                let normalized = cosine::preprocess(c.clone());
                centroids_flat.extend_from_slice(&normalized);
                centroid_vecs.push(normalized);
            }

            let num_centroids = clustering_result.num_clusters;

            // Primary assignment
            let mut posting_assignments: Vec<u32> = clustering_result
                .assignments
                .iter()
                .map(|&a| a as u32)
                .collect();
            let mut original_indices: Vec<u32> =
                (0..num_original_vectors).map(|i| i as u32).collect();

            // Boundary replication: for each vector, check if the 2nd-nearest
            // centroid is within (1 + epsilon) of the nearest. If so, replicate
            // the vector into that posting by appending a copy.
            // Collect boundary replication candidates first (to avoid borrow issues)
            let mut replicas: Vec<(Vec<f32>, SegmentPayload, u32, u32)> = Vec::new();
            for i in 0..num_original_vectors {
                let vec_start = i * dimension;
                let vec_slice = &vectors[vec_start..vec_start + dimension];
                let primary = posting_assignments[i] as usize;

                // Find 2nd-nearest centroid
                let primary_sim = cosine::cosine_similarity(vec_slice, &centroid_vecs[primary]);
                let primary_dist = 1.0 - primary_sim;

                let mut second_best_dist = f32::MAX;
                let mut second_best_idx = primary;

                for (c, centroid) in centroid_vecs.iter().enumerate() {
                    if c == primary {
                        continue;
                    }
                    let sim = cosine::cosine_similarity(vec_slice, centroid);
                    let dist = 1.0 - sim;
                    if dist < second_best_dist {
                        second_best_dist = dist;
                        second_best_idx = c;
                    }
                }

                // Mark for replication if within epsilon threshold
                if second_best_idx != primary
                    && primary_dist > 0.0
                    && second_best_dist <= primary_dist * (1.0 + BOUNDARY_EPSILON)
                {
                    replicas.push((
                        vec_slice.to_vec(),
                        payloads[i].clone(),
                        second_best_idx as u32,
                        i as u32,
                    ));
                }
            }

            // Apply replications
            let replicated_count = replicas.len();
            for (vec_data, payload, assignment, original_index) in replicas {
                vectors.extend_from_slice(&vec_data);
                payloads.push(payload);
                posting_assignments.push(assignment);
                original_indices.push(original_index);
            }

            let num_total_vectors = num_original_vectors + replicated_count;

            SpannSegment {
                vectors: VectorData::Owned(vectors),
                payloads,
                dimension,
                num_original_vectors,
                num_total_vectors,
                centroids: VectorData::Owned(centroids_flat),
                num_centroids,
                vectors_f16_mmap: None,
                posting_assignments,
                original_indices,
                deleted_bitset: None,
            }
        }
    }

    /// Save segment data to a directory.
    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(dir)?;

        let vectors_path = dir.join(VECTORS_FILENAME);
        let mut f = BufWriter::new(File::create(&vectors_path)?);
        f.write_all(&(self.dimension as u32).to_le_bytes())?;
        f.write_all(&(self.num_original_vectors as u32).to_le_bytes())?;
        let vectors_slice = self.vectors.as_f32_slice();
        let vectors_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                vectors_slice.as_ptr() as *const u8,
                vectors_slice.len() * std::mem::size_of::<f32>(),
            )
        };
        f.write_all(vectors_bytes)?;
        f.into_inner()?.sync_all()?;

        let vectors_f16_path = dir.join(VECTORS_F16_FILENAME);
        let mut f = BufWriter::new(File::create(&vectors_f16_path)?);
        let f16_bytes = crate::spann::quantization::quantize_f32_to_f16(vectors_slice);
        f.write_all(&f16_bytes)?;
        f.into_inner()?.sync_all()?;

        let payloads_path = dir.join(PAYLOADS_FILENAME);
        let mut f = BufWriter::new(File::create(&payloads_path)?);
        for payload in &self.payloads {
            let serializable: BTreeMap<String, String> = payload
                .filter_fields
                .iter()
                .map(|(k, v)| (String::from(k.clone()), base64::encode_urlsafe(v)))
                .collect();
            let entry = serde_json::json!({
                "internal_id": String::from(payload.internal_id),
                "filter_fields": serializable,
                "timestamp": payload.timestamp,
            });
            serde_json::to_writer(&mut f, &entry)?;
            f.write_all(b"\n")?;
        }
        f.into_inner()?.sync_all()?;

        if self.num_centroids > 0 {
            let centroids_path = dir.join(CENTROIDS_FILENAME);
            let mut f = BufWriter::new(File::create(&centroids_path)?);
            f.write_all(&(self.num_centroids as u32).to_le_bytes())?;
            let centroids_slice = self.centroids.as_f32_slice();
            let centroids_bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    centroids_slice.as_ptr() as *const u8,
                    centroids_slice.len() * std::mem::size_of::<f32>(),
                )
            };
            f.write_all(centroids_bytes)?;
            f.into_inner()?.sync_all()?;

            let postings_path = dir.join(POSTINGS_FILENAME);
            let mut f = BufWriter::new(File::create(&postings_path)?);
            let assignments_bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    self.posting_assignments.as_ptr() as *const u8,
                    self.posting_assignments.len() * std::mem::size_of::<u32>(),
                )
            };
            f.write_all(assignments_bytes)?;
            f.into_inner()?.sync_all()?;

            let originals_path = dir.join(ORIGINALS_FILENAME);
            let mut f = BufWriter::new(File::create(&originals_path)?);
            let originals_bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    self.original_indices.as_ptr() as *const u8,
                    self.original_indices.len() * std::mem::size_of::<u32>(),
                )
            };
            f.write_all(originals_bytes)?;
            f.into_inner()?.sync_all()?;
        }

        Ok(())
    }

    /// Load segment data from a directory.
    ///
    /// Uses memory-mapping for the vectors file to leverage the kernel's page
    /// cache and avoid unnecessary copies for large segments.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let vectors_path = dir.join(VECTORS_FILENAME);
        let vectors_file = File::open(&vectors_path)?;
        let vectors_mmap = unsafe { memmap2::Mmap::map(&vectors_file)? };

        if vectors_mmap.len() < 8 {
            return Err(anyhow::anyhow!(
                "invalid vectors.bin: header is too short ({} bytes)",
                vectors_mmap.len()
            ));
        }

        let header = &vectors_mmap[0..8];
        let dimension = u32::from_le_bytes(header[0..4].try_into()?) as usize;
        let num_original_vectors = u32::from_le_bytes(header[4..8].try_into()?) as usize;

        let vector_data = &vectors_mmap[8..];
        if vector_data.len() % std::mem::size_of::<f32>() != 0 {
            return Err(anyhow::anyhow!(
                "invalid vectors.bin: vector byte length {} is not divisible by {}",
                vector_data.len(),
                std::mem::size_of::<f32>()
            ));
        }
        let num_floats = vector_data.len() / std::mem::size_of::<f32>();
        if dimension == 0 && num_floats != 0 {
            return Err(anyhow::anyhow!(
                "invalid vectors.bin: dimension is 0 but vector payload is non-empty"
            ));
        }
        if dimension > 0 && num_floats % dimension != 0 {
            return Err(anyhow::anyhow!(
                "invalid vectors.bin: float count {} is not divisible by dimension {}",
                num_floats,
                dimension
            ));
        }

        let num_total_vectors = if dimension == 0 {
            0
        } else {
            num_floats / dimension
        };
        // Zero-copy: keep the mmap alive and serve vector reads directly from it.
        // The kernel handles paging the data in/out of physical memory as needed.
        let vectors = VectorData::Mmap {
            mmap: vectors_mmap,
            data_offset: 8, // skip the 8-byte header (dim u32 + count u32)
            num_floats,
        };
        let vectors_f16_path = dir.join(VECTORS_F16_FILENAME);
        let vectors_f16_mmap = if vectors_f16_path.exists() {
            let f16_file = File::open(&vectors_f16_path)?;
            let f16_mmap = unsafe { memmap2::Mmap::map(&f16_file)? };
            let expected_f16_bytes = num_floats * std::mem::size_of::<u16>();
            if f16_mmap.len() != expected_f16_bytes {
                return Err(anyhow::anyhow!(
                    "invalid vectors_q.f16: expected {} bytes, got {} bytes",
                    expected_f16_bytes,
                    f16_mmap.len()
                ));
            }
            Some(f16_mmap)
        } else {
            None
        };

        let payloads_path = dir.join(PAYLOADS_FILENAME);
        let content = fs::read_to_string(&payloads_path)?;
        let mut payloads = Vec::with_capacity(num_total_vectors);
        for line in content.lines() {
            if line.is_empty() {
                continue;
            }
            let entry: serde_json::Value = serde_json::from_str(line)?;
            let internal_id = match entry["internal_id"].as_str() {
                Some(id) => id.parse()?,
                // Fallback for older payload files that didn't persist ids.
                None => InternalId((payloads.len() as u128).to_le_bytes()),
            };
            let filter_fields_raw = entry["filter_fields"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            let field_path: FieldPath = k.parse().unwrap();
                            let bytes = base64::decode_urlsafe(v.as_str().unwrap()).unwrap();
                            (field_path, bytes)
                        })
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default();
            let timestamp = entry["timestamp"].as_u64().unwrap_or(0);
            payloads.push(SegmentPayload {
                internal_id,
                filter_fields: filter_fields_raw,
                timestamp,
            });
        }

        let centroids_path = dir.join(CENTROIDS_FILENAME);
        let (centroids, num_centroids, posting_assignments, original_indices) =
            if centroids_path.exists() {
                let centroids_file = File::open(&centroids_path)?;
                let centroids_mmap = unsafe { memmap2::Mmap::map(&centroids_file)? };
                if centroids_mmap.len() < std::mem::size_of::<u32>() {
                    return Err(anyhow::anyhow!(
                        "invalid centroids.bin: header is too short ({} bytes)",
                        centroids_mmap.len()
                    ));
                }
                let num_centroids = u32::from_le_bytes(centroids_mmap[0..4].try_into()?) as usize;
                let num_centroid_floats = num_centroids * dimension;
                let expected_centroid_bytes = num_centroid_floats * std::mem::size_of::<f32>();
                let centroid_payload_len = centroids_mmap.len() - std::mem::size_of::<u32>();
                if centroid_payload_len % std::mem::size_of::<f32>() != 0 {
                    return Err(anyhow::anyhow!(
                        "invalid centroids.bin: centroid payload length {} is not divisible by {}",
                        centroid_payload_len,
                        std::mem::size_of::<f32>()
                    ));
                }
                if centroid_payload_len < expected_centroid_bytes {
                    return Err(anyhow::anyhow!(
                        "invalid centroids.bin: expected at least {} centroid bytes, got {} bytes",
                        expected_centroid_bytes,
                        centroid_payload_len
                    ));
                }
                let centroids = VectorData::Mmap {
                    mmap: centroids_mmap,
                    data_offset: 4, // skip centroid count u32 header
                    num_floats: num_centroid_floats,
                };

                let postings_path = dir.join(POSTINGS_FILENAME);
                let mut f = BufReader::new(File::open(&postings_path)?);
                let mut posting_assignments = vec![0u32; num_total_vectors];
                let assignments_bytes: &mut [u8] = unsafe {
                    std::slice::from_raw_parts_mut(
                        posting_assignments.as_mut_ptr() as *mut u8,
                        num_total_vectors * std::mem::size_of::<u32>(),
                    )
                };
                f.read_exact(assignments_bytes)?;

                let originals_path = dir.join(ORIGINALS_FILENAME);
                let mut f = BufReader::new(File::open(&originals_path)?);
                let mut original_indices = vec![0u32; num_total_vectors];
                let originals_bytes: &mut [u8] = unsafe {
                    std::slice::from_raw_parts_mut(
                        original_indices.as_mut_ptr() as *mut u8,
                        num_total_vectors * std::mem::size_of::<u32>(),
                    )
                };
                f.read_exact(originals_bytes)?;

                (
                    centroids,
                    num_centroids,
                    posting_assignments,
                    original_indices,
                )
            } else {
                (
                    VectorData::Owned(Vec::new()),
                    0,
                    Vec::new(),
                    (0..num_total_vectors).map(|i| i as u32).collect(),
                )
            };

        Ok(SpannSegment {
            vectors,
            payloads,
            dimension,
            num_original_vectors,
            num_total_vectors,
            centroids,
            num_centroids,
            vectors_f16_mmap,
            posting_assignments,
            original_indices,
            deleted_bitset: None,
        })
    }

    fn is_deleted(&self, index: u32) -> bool {
        if let Some(ref bitset) = self.deleted_bitset {
            // The deleted bitset is indexed by the original point offset
            // (the id_tracker offset), which corresponds to the original_indices
            // value for this vector slot.
            let original = if self.original_indices.is_empty() {
                index
            } else {
                self.original_indices[index as usize]
            };
            bitset.is_deleted(original)
        } else {
            false
        }
    }

    pub fn num_vectors(&self) -> usize {
        self.num_original_vectors
    }

    pub fn num_total_vectors(&self) -> usize {
        self.num_total_vectors
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    pub fn internal_id_for_vector(&self, internal_id: u32) -> Option<InternalId> {
        self.payloads
            .get(internal_id as usize)
            .map(|payload| payload.internal_id)
    }
}

/// Snapshot a segment into a tar file + id tracker files.
pub fn snapshot_segment(
    id_tracker: &mut VectorMemoryIdTracker,
    segment: &SpannSegment,
    tmp_path: &Path,
    index_path: &Path,
) -> anyhow::Result<VectorDiskSegmentPaths> {
    // Save segment to a temp directory, then tar it
    let segment_dir = tmp_path.join("segment_data");
    segment.save(&segment_dir)?;

    // Create tar
    let tar_path = index_path.join("segment.tar");
    {
        let tar_file = File::create(&tar_path)?;
        let mut tar_builder = tar::Builder::new(tar_file);
        tar_builder.append_dir_all(".", &segment_dir)?;
        tar_builder.finish()?;
    }

    // Write ID tracker files
    let uuids_path = index_path.join(UUID_TABLE_FILENAME);
    {
        let mut out = BufWriter::new(File::create(&uuids_path)?);
        id_tracker.write_uuids(&mut out)?;
        out.into_inner()?.sync_all()?;
    }
    let deleted_bitset_path = index_path.join(DELETED_BITSET_FILENAME);
    {
        let mut out = BufWriter::new(File::create(&deleted_bitset_path)?);
        id_tracker.write_deleted_bitset(&mut out)?;
        out.into_inner()?.sync_all()?;
    }

    Ok(VectorDiskSegmentPaths {
        segment: tar_path,
        uuids: uuids_path,
        deleted_bitset: deleted_bitset_path,
    })
}

/// Restore a segment from a tar archive.
pub async fn restore_segment_from_tar(archive_path: &Path) -> anyhow::Result<PathBuf> {
    let segment_id = archive_path
        .file_stem()
        .and_then(|f| f.to_str())
        .unwrap()
        .to_owned();

    let out_path = archive_path
        .parent()
        .expect("Failed to obtain parent for archive")
        .join(&segment_id);

    let archive_path = archive_path.to_owned();
    tokio_spawn_blocking("segment_restore_from_tar", move || {
        fs::create_dir_all(&out_path)?;
        let file = File::open(&archive_path)?;
        let mut archive = tar::Archive::new(file);
        archive.unpack(&out_path)?;
        Ok(out_path)
    })
    .await?
}

/// Load a disk segment from untarred paths, including the deleted bitset
/// for filtering out deleted vectors during search.
pub fn load_disk_segment(paths: UntarredVectorDiskSegmentPaths) -> anyhow::Result<SpannSegment> {
    let mut segment = SpannSegment::load(&paths.segment_dir)?;
    // Load the deleted bitset so search can skip deleted vectors
    let deleted = DeletedBitset::load_from_path(&paths.deleted_bitset)?;
    segment.deleted_bitset = Some(deleted);
    Ok(segment)
}

/// Load a disk segment for testing (handles tar extraction).
#[cfg(any(test, feature = "testing"))]
pub async fn unsafe_load_disk_segment(
    paths: &VectorDiskSegmentPaths,
) -> anyhow::Result<SpannSegment> {
    let path = restore_segment_from_tar(&paths.segment).await?;
    let untarred = UntarredVectorDiskSegmentPaths::from(path, paths.clone());
    load_disk_segment(untarred)
}

/// Build a segment, snapshot it, restore it, and return the loaded segment.
/// This is useful for testing the full round-trip.
#[cfg(any(test, feature = "testing"))]
pub fn build_and_snapshot_segment(
    documents: Vec<(InternalId, IndexedVector, BTreeMap<FieldPath, Vec<u8>>, u64)>,
    dimension: usize,
    tmp_path: &Path,
    disk_path: &Path,
) -> anyhow::Result<VectorDiskSegmentValues> {
    let mut id_tracker = VectorMemoryIdTracker::new();
    for (i, (id, ..)) in documents.iter().enumerate() {
        id_tracker.set_link(*id, i as u32)?;
    }

    let segment = SpannSegment::build(documents, dimension);
    let num_vectors = segment.num_vectors() as u32;
    let paths = snapshot_segment(&mut id_tracker, &segment, tmp_path, disk_path)?;

    Ok(VectorDiskSegmentValues {
        paths,
        num_vectors,
        num_deleted: 0,
    })
}

/// Merge multiple segments into a single new segment.
pub fn merge_disk_segments(
    segments: Vec<&SpannSegment>,
    id_trackers: Vec<&VectorStaticIdTracker>,
    dimension: usize,
    tmp_path: &Path,
    disk_path: &Path,
) -> anyhow::Result<VectorDiskSegmentValues> {
    // Collect all non-deleted vectors from all segments
    let mut documents = Vec::new();
    let mut memory_tracker = VectorMemoryIdTracker::new();
    let mut offset: u32 = 0;

    for (segment, tracker) in segments.iter().zip(id_trackers.iter()) {
        for (internal_id, convex_id) in tracker.iter_non_deleted() {
            let vec_start = internal_id as usize * dimension;
            let vec_end = vec_start + dimension;
            let vectors = segment.vectors.as_f32_slice();
            let vector_data = &vectors[vec_start..vec_end];
            let payload = &segment.payloads[internal_id as usize];

            // Note: vectors are already normalized in the segment
            let vector = IndexedVector::try_from(vector_data.to_vec())?;
            documents.push((
                convex_id,
                vector,
                payload.filter_fields.clone(),
                payload.timestamp,
            ));

            memory_tracker.set_link(convex_id, offset)?;
            offset += 1;
        }
    }

    let num_vectors = documents.len() as u32;
    let built_segment = SpannSegment::build(documents, dimension);

    let paths = snapshot_segment(&mut memory_tracker, &built_segment, tmp_path, disk_path)?;

    Ok(VectorDiskSegmentValues {
        paths,
        num_vectors,
        num_deleted: 0,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::TempDir;
    use value::{
        FieldPath,
        InternalId,
    };

    use super::*;
    use crate::IndexedVector;

    fn make_test_vector(dim: usize, seed: f32) -> IndexedVector {
        let v: Vec<f32> = (0..dim).map(|i| seed + i as f32).collect();
        IndexedVector::try_from(v).unwrap()
    }

    fn make_test_id(n: u128) -> InternalId {
        InternalId(n.to_le_bytes())
    }

    #[test]
    fn test_segment_build_and_search() {
        let dim = 4;
        let docs: Vec<_> = (0..10)
            .map(|i| {
                (
                    make_test_id(i as u128),
                    make_test_vector(dim, i as f32),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);
        assert_eq!(segment.num_vectors(), 10);
        assert_eq!(segment.dimension(), dim);

        // Search for a vector similar to doc 0
        let query = vec![0.0, 1.0, 2.0, 3.0];
        let results = segment.search(&query, &BTreeMap::new(), 3);
        assert_eq!(results.len(), 3);
        // Results should be sorted by score descending
        assert!(results[0].1 >= results[1].1);
        assert!(results[1].1 >= results[2].1);
    }

    #[test]
    fn test_segment_save_load_roundtrip() {
        let dim = 4;
        let docs: Vec<_> = (0..5)
            .map(|i| {
                let mut filter = BTreeMap::new();
                if i % 2 == 0 {
                    filter.insert("color".parse::<FieldPath>().unwrap(), vec![1u8, 2]);
                }
                (
                    make_test_id(i as u128),
                    make_test_vector(dim, i as f32 * 10.0),
                    filter,
                    100 + i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);
        let tmpdir = TempDir::new().unwrap();
        let save_dir = tmpdir.path().join("segment");

        segment.save(&save_dir).unwrap();
        let loaded = SpannSegment::load(&save_dir).unwrap();

        assert_eq!(loaded.num_vectors(), 5);
        assert_eq!(loaded.dimension(), dim);
        assert!(loaded.vectors_f16_mmap.is_some());

        // Search should return same results on loaded segment
        let query = vec![0.0, 10.0, 20.0, 30.0];
        let original_results = segment.search(&query, &BTreeMap::new(), 3);
        let loaded_results = loaded.search(&query, &BTreeMap::new(), 3);
        assert_eq!(original_results.len(), loaded_results.len());
        for (orig, load) in original_results.iter().zip(loaded_results.iter()) {
            assert_eq!(orig.0, load.0); // same point index
            assert!((orig.1 - load.1).abs() < 5e-3); // close score
            assert_eq!(orig.2, load.2); // same timestamp
        }
    }

    #[test]
    fn test_segment_filter() {
        let dim = 3;
        let color_field: FieldPath = "color".parse().unwrap();
        let docs: Vec<_> = (0..6)
            .map(|i| {
                let mut filter = BTreeMap::new();
                let color_val = if i < 3 { vec![1u8] } else { vec![2u8] };
                filter.insert(color_field.clone(), color_val);
                (
                    make_test_id(i as u128),
                    make_test_vector(dim, i as f32),
                    filter,
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        // Filter for color == [1]
        let mut conditions = BTreeMap::new();
        conditions.insert(
            color_field.clone(),
            crate::query::CompiledVectorFilter::Eq(vec![1u8]),
        );

        let query = vec![0.0, 1.0, 2.0];
        let results = segment.search(&query, &conditions, 10);
        // Should only find docs 0, 1, 2 (color == [1])
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_snapshot_and_restore() {
        let dim = 4;
        let docs: Vec<_> = (0..3)
            .map(|i| {
                (
                    make_test_id(i as u128),
                    make_test_vector(dim, i as f32),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let tmpdir = TempDir::new().unwrap();
        let tmp_path = tmpdir.path().join("tmp");
        let disk_path = tmpdir.path().join("disk");
        std::fs::create_dir_all(&tmp_path).unwrap();
        std::fs::create_dir_all(&disk_path).unwrap();

        let result = build_and_snapshot_segment(docs, dim, &tmp_path, &disk_path).unwrap();
        assert_eq!(result.num_vectors, 3);

        // Restore from tar and load
        let loaded = unsafe_load_disk_segment(&result.paths).await.unwrap();
        assert_eq!(loaded.num_vectors(), 3);

        // Search on loaded segment
        let query = vec![0.0, 1.0, 2.0, 3.0];
        let results = loaded.search(&query, &BTreeMap::new(), 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_empty_segment() {
        let dim = 4;
        let segment = SpannSegment::build(vec![], dim);
        assert_eq!(segment.num_vectors(), 0);

        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = segment.search(&query, &BTreeMap::new(), 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_spann_centroid_routing() {
        // Build a segment large enough to trigger clustering
        let dim = 8;
        let n = 500; // > CLUSTERING_THRESHOLD (256)
        let mut rng = rand::rng();
        use rand::Rng;

        let docs: Vec<_> = (0..n)
            .map(|i| {
                let v: Vec<f32> = (0..dim).map(|_| rng.random::<f32>()).collect();
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(v).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        // Save the raw vectors for brute-force comparison
        let raw_vectors: Vec<Vec<f32>> =
            docs.iter().map(|(_, v, ..)| Vec::from(v.clone())).collect();

        let segment = SpannSegment::build(docs, dim);

        // Should have centroids (n > CLUSTERING_THRESHOLD)
        assert!(
            segment.num_centroids > 0,
            "Large segment should have centroids, got {}",
            segment.num_centroids
        );
        assert_eq!(segment.num_vectors(), n);
        assert_eq!(
            segment.posting_assignments.len(),
            segment.num_total_vectors()
        );
        assert_eq!(segment.original_indices.len(), segment.num_total_vectors());
        assert!(segment.num_total_vectors() >= n);

        // Search with centroid routing
        let query: Vec<f32> = (0..dim).map(|_| rng.random::<f32>()).collect();
        let spann_results = segment.search(&query, &BTreeMap::new(), 10);
        assert_eq!(spann_results.len(), 10);

        // Brute-force search for ground truth
        let query_norm = cosine::preprocess(query.clone());
        let mut brute_force: Vec<(usize, f32)> = raw_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let v_norm = cosine::preprocess(v.clone());
                (i, cosine::cosine_similarity(&query_norm, &v_norm))
            })
            .collect();
        brute_force.sort_by(|a, b| b.1.total_cmp(&a.1));

        // Compute recall@10
        let top10_true: std::collections::HashSet<usize> =
            brute_force.iter().take(10).map(|(i, _)| *i).collect();
        let top10_spann: std::collections::HashSet<usize> = spann_results
            .iter()
            .map(|(idx, ..)| *idx as usize)
            .collect();
        let recall = top10_true.intersection(&top10_spann).count() as f32 / 10.0;

        // With nprobe=32 on 500 vectors (5 clusters), recall should be high
        assert!(
            recall >= 0.7,
            "Recall@10 should be >= 0.7, got {recall} (nprobe={DEFAULT_NPROBE})"
        );
    }

    #[test]
    fn test_spann_save_load_with_centroids() {
        // Build a segment large enough to trigger clustering
        let dim = 4;
        let n = 300; // > CLUSTERING_THRESHOLD (256)

        let docs: Vec<_> = (0..n)
            .map(|i| {
                let v: Vec<f32> = (0..dim).map(|d| (i * dim + d) as f32).collect();
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(v).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);
        assert!(segment.num_centroids > 0);

        let tmpdir = TempDir::new().unwrap();
        let save_dir = tmpdir.path().join("spann_segment");
        segment.save(&save_dir).unwrap();

        let loaded = SpannSegment::load(&save_dir).unwrap();
        assert_eq!(loaded.num_vectors(), n);
        assert_eq!(loaded.num_total_vectors(), segment.num_total_vectors());
        assert_eq!(loaded.num_centroids, segment.num_centroids);
        assert_eq!(loaded.posting_assignments, segment.posting_assignments);
        assert_eq!(loaded.original_indices, segment.original_indices);
        assert!(loaded.vectors_f16_mmap.is_some());

        // f16 scoring may slightly perturb ranking; verify strong overlap.
        let query = vec![1.0, 2.0, 3.0, 4.0];
        let original_results = segment.search(&query, &BTreeMap::new(), 5);
        let loaded_results = loaded.search(&query, &BTreeMap::new(), 5);
        assert_eq!(original_results.len(), loaded_results.len());

        let original_ids: std::collections::HashSet<u32> =
            original_results.iter().map(|(id, ..)| *id).collect();
        let loaded_ids: std::collections::HashSet<u32> =
            loaded_results.iter().map(|(id, ..)| *id).collect();
        let overlap = original_ids.intersection(&loaded_ids).count();
        assert!(
            overlap >= 4,
            "Expected >=4 overlap in top-5, got {overlap}"
        );

        for (id, score, _) in &original_results {
            if let Some((_, loaded_score, _)) = loaded_results.iter().find(|(i, ..)| i == id) {
                assert!((score - loaded_score).abs() < 1e-2);
            }
        }
    }

    #[test]
    fn test_single_vector_segment() {
        let dim = 3;
        let docs = vec![(
            make_test_id(1),
            make_test_vector(dim, 1.0),
            BTreeMap::new(),
            42u64,
        )];

        let segment = SpannSegment::build(docs, dim);
        assert_eq!(segment.num_vectors(), 1);

        let query = vec![1.0, 2.0, 3.0];
        let results = segment.search(&query, &BTreeMap::new(), 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].2, 42);
    }
}
