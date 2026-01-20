#![feature(type_alias_impl_trait)]
#![feature(try_blocks)]
#![feature(btree_extract_if)]

mod metrics;
mod state;
pub mod binary;
pub mod worker;

pub use worker::{
    SyncWorker,
    SyncWorkerConfig,
};

#[cfg(test)]
mod tests;

/// ServerMessage using PackedSyncValue for zero-copy transfer
pub type ServerMessage = sync_types::ServerMessage<packed_value::PackedSyncValue>;
