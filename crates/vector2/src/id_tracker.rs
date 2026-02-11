use std::io::Write;

use common::{
    deleted_bitset::DeletedBitset,
    id_tracker::{
        MemoryIdTracker,
        StaticIdTracker,
    },
};
use value::InternalId;

/// Qdrant-compatible operation number; always 1.
pub const OP_NUM: u64 = 1;

/// Mutable ID tracker for building segments. Maps between external UUIDs
/// (derived from InternalId) and dense internal offsets.
pub struct VectorMemoryIdTracker {
    pub(crate) memory_id_tracker: MemoryIdTracker,
    pub(crate) deleted: DeletedBitset,
}

impl VectorMemoryIdTracker {
    pub fn new() -> Self {
        Self {
            memory_id_tracker: MemoryIdTracker::default(),
            deleted: DeletedBitset::new(0),
        }
    }

    pub fn total_point_count(&self) -> usize {
        self.memory_id_tracker.by_index_id.len()
    }

    pub fn deleted_point_count(&self) -> usize {
        self.deleted.num_deleted()
    }

    pub fn available_point_count(&self) -> usize {
        self.total_point_count() - self.deleted_point_count()
    }

    pub fn is_deleted(&self, internal_id: u32) -> bool {
        self.deleted.is_deleted(internal_id)
    }

    pub fn set_link(&mut self, id: InternalId, internal_id: u32) -> anyhow::Result<()> {
        self.memory_id_tracker.insert(internal_id, id.0);
        self.deleted.resize(internal_id as usize + 1);
        Ok(())
    }

    pub fn delete(&mut self, id: InternalId) -> anyhow::Result<()> {
        let internal_id =
            self.memory_id_tracker
                .index_id(id.0)
                .ok_or_else(|| anyhow::anyhow!("Unknown id for deletion"))? as u32;
        self.deleted.delete(internal_id)?;
        Ok(())
    }

    pub fn internal_id(&self, id: InternalId) -> Option<u32> {
        self.memory_id_tracker.index_id(id.0).map(|ix| ix as u32)
    }

    pub fn external_id(&self, internal_id: u32) -> Option<InternalId> {
        if self.is_deleted(internal_id) {
            return None;
        }
        self.memory_id_tracker
            .convex_id(internal_id)
            .map(InternalId)
    }

    pub fn iter_non_deleted(&self) -> impl Iterator<Item = (u32, InternalId)> + '_ {
        self.memory_id_tracker
            .by_index_id
            .iter()
            .filter(|(internal_id, _)| !self.deleted.is_deleted(**internal_id))
            .map(|(internal_id, convex_id)| (*internal_id, InternalId(*convex_id)))
    }

    pub fn write_uuids(&mut self, out: impl Write) -> anyhow::Result<()> {
        self.memory_id_tracker.write_id_tracker(out)
    }

    pub fn write_deleted_bitset(&mut self, out: impl Write) -> anyhow::Result<()> {
        self.deleted.write(out)
    }
}

/// Immutable ID tracker for query-time segments. Loaded from serialized files.
pub struct VectorStaticIdTracker {
    pub id_tracker: StaticIdTracker,
    pub deleted_bitset: DeletedBitset,
}

impl VectorStaticIdTracker {
    pub fn total_point_count(&self) -> usize {
        self.id_tracker.count()
    }

    pub fn deleted_point_count(&self) -> usize {
        self.deleted_bitset.num_deleted()
    }

    pub fn is_deleted(&self, internal_id: u32) -> bool {
        self.deleted_bitset.is_deleted(internal_id)
    }

    pub fn internal_id(&self, id: InternalId) -> Option<u32> {
        self.id_tracker.lookup(id.0)
    }

    pub fn external_id(&self, internal_id: u32) -> Option<InternalId> {
        if self.is_deleted(internal_id) {
            return None;
        }
        self.id_tracker
            .get_convex_id(internal_id as usize)
            .map(InternalId)
    }

    pub fn iter_non_deleted(&self) -> impl Iterator<Item = (u32, InternalId)> + '_ {
        (0..self.id_tracker.count() as u32)
            .filter(|internal_id| !self.deleted_bitset.is_deleted(*internal_id))
            .filter_map(|internal_id| {
                self.id_tracker
                    .get_convex_id(internal_id as usize)
                    .map(|convex_id| (internal_id, InternalId(convex_id)))
            })
    }
}
