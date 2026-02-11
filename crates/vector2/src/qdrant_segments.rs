//! Backward-compatible re-exports for code that imports
//! `vector::qdrant_segments::*`. These all delegate to the new `spann::segment`
//! module.

use std::path::Path;

use common::{
    deleted_bitset::DeletedBitset,
    id_tracker::StaticIdTracker,
};

#[cfg(any(test, feature = "testing"))]
pub use crate::spann::segment::unsafe_load_disk_segment;
pub use crate::spann::segment::{
    load_disk_segment,
    restore_segment_from_tar,
    SpannSegment,
    UntarredVectorDiskSegmentPaths,
    VectorDiskSegmentPaths,
    VectorDiskSegmentValues,
};
use crate::{
    id_tracker::VectorStaticIdTracker,
    spann::segment,
};

/// Backward-compatible merge function that takes paths + segments (matching the
/// old qdrant-based API). Loads id trackers from the paths and delegates to the
/// new `merge_disk_segments`.
pub fn merge_disk_segments_hnsw(
    segments: Vec<(Option<UntarredVectorDiskSegmentPaths>, &SpannSegment)>,
    dimension: usize,
    tmp_path: &Path,
    disk_path: &Path,
) -> anyhow::Result<VectorDiskSegmentValues> {
    let mut spann_segments: Vec<&SpannSegment> = Vec::new();
    let mut owned_trackers: Vec<VectorStaticIdTracker> = Vec::new();

    for (paths, seg) in &segments {
        spann_segments.push(*seg);
        let p = paths
            .as_ref()
            .expect("merge_disk_segments_hnsw requires paths for id tracker loading");
        let tracker = VectorStaticIdTracker {
            id_tracker: StaticIdTracker::load_from_path(&p.uuids)?,
            deleted_bitset: DeletedBitset::load_from_path(&p.deleted_bitset)?,
        };
        owned_trackers.push(tracker);
    }

    let tracker_refs: Vec<&VectorStaticIdTracker> = owned_trackers.iter().collect();
    segment::merge_disk_segments(spann_segments, tracker_refs, dimension, tmp_path, disk_path)
}
