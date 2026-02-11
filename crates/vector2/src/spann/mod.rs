pub mod clustering;
pub mod cosine;
pub mod quantization;
mod schema;
pub mod segment;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "testing"))]
pub use self::{
    schema::cosine_similarity,
    segment::unsafe_load_disk_segment,
};
pub use self::{
    schema::VectorSchema,
    segment::{
        load_disk_segment,
        merge_disk_segments,
        restore_segment_from_tar,
        ExternalId,
        NormalizedVectorDocument,
        PreviousSegmentsHack,
        SpannSegment,
        UntarredVectorDiskSegmentPaths,
        VectorDiskSegmentPaths,
        VectorDiskSegmentValues,
        VectorDocument,
    },
};
