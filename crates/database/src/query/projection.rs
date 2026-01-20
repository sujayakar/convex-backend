use std::collections::BTreeMap;

use async_trait::async_trait;
use common::{
    document::{
        DeveloperDocument,
        CREATION_TIME_FIELD,
        ID_FIELD,
    },
    query::CursorPosition,
    runtime::Runtime,
    types::{
        IndexName,
        TabletIndexName,
    },
};
use value::{
    ConvexObject,
    FieldName,
    FieldPath,
};

use super::{
    DeveloperIndexRangeResponse,
    QueryNode,
    QueryStream,
    QueryStreamNext,
};
use crate::Transaction;

/// Applies field projection to documents before they are passed to downstream
/// nodes (like Filter). This ensures that filters only see the projected fields,
/// providing consistent semantics regardless of caching.
pub(super) struct Projection {
    inner: QueryNode,
    fields: Vec<FieldPath>,
}

impl Projection {
    pub fn new(inner: QueryNode, fields: Vec<FieldPath>) -> Self {
        Self { inner, fields }
    }

    fn project_document(&self, doc: DeveloperDocument) -> anyhow::Result<DeveloperDocument> {
        let value = doc.value().0.clone();
        let mut new_fields = BTreeMap::new();

        // Always include system fields
        if let Some(id_val) = value.get(&FieldName::from(ID_FIELD.clone())) {
            new_fields.insert(FieldName::from(ID_FIELD.clone()), id_val.clone());
        }
        if let Some(ct_val) = value.get(&FieldName::from(CREATION_TIME_FIELD.clone())) {
            new_fields.insert(FieldName::from(CREATION_TIME_FIELD.clone()), ct_val.clone());
        }

        // Include selected fields (skip if not present in document)
        for field_path in &self.fields {
            // For now, only support top-level fields
            if field_path.fields().len() != 1 {
                continue;
            }
            let field_name = FieldName::from(field_path.fields()[0].clone());
            // Skip system fields (already added above)
            if field_name == FieldName::from(ID_FIELD.clone())
                || field_name == FieldName::from(CREATION_TIME_FIELD.clone())
            {
                continue;
            }
            if let Some(v) = value.get(&field_name) {
                new_fields.insert(field_name, v.clone());
            }
        }

        let new_value = ConvexObject::try_from(new_fields)?;
        Ok(DeveloperDocument::new(
            doc.id(),
            doc.creation_time(),
            new_value,
        ))
    }
}

#[async_trait]
impl QueryStream for Projection {
    fn cursor_position(&self) -> &Option<CursorPosition> {
        self.inner.cursor_position()
    }

    fn split_cursor_position(&self) -> Option<&CursorPosition> {
        self.inner.split_cursor_position()
    }

    fn is_approaching_data_limit(&self) -> bool {
        self.inner.is_approaching_data_limit()
    }

    async fn next<RT: Runtime>(
        &mut self,
        tx: &mut Transaction<RT>,
        prefetch_hint: Option<usize>,
    ) -> anyhow::Result<QueryStreamNext> {
        match self.inner.next(tx, prefetch_hint).await? {
            QueryStreamNext::Ready(Some((document, write_timestamp))) => {
                let projected = self.project_document(document)?;
                Ok(QueryStreamNext::Ready(Some((projected, write_timestamp))))
            },
            QueryStreamNext::Ready(None) => Ok(QueryStreamNext::Ready(None)),
            QueryStreamNext::WaitingOn(request) => Ok(QueryStreamNext::WaitingOn(request)),
        }
    }

    fn feed(&mut self, index_range_response: DeveloperIndexRangeResponse) -> anyhow::Result<()> {
        self.inner.feed(index_range_response)
    }

    fn tablet_index_name(&self) -> Option<&TabletIndexName> {
        self.inner.tablet_index_name()
    }

    fn printable_index_name(&self) -> &IndexName {
        self.inner.printable_index_name()
    }
}
