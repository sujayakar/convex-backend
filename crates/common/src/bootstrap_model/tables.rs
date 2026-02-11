use std::sync::LazyLock;

use serde::{
    Deserialize,
    Serialize,
};
use value::{
    codegen_convex_serialization,
    TableNamespace,
    TableNumber,
};

use crate::types::{
    FieldName,
    TableName,
};

pub static TABLES_TABLE: LazyLock<TableName> =
    LazyLock::new(|| "_tables".parse().expect("Invalid built-in tables table"));

pub static NAME_FIELD: LazyLock<FieldName> =
    LazyLock::new(|| "name".parse().expect("Invalid name field"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(any(test, feature = "testing"), derive(proptest_derive::Arbitrary))]
pub enum TableState {
    /// The table exists. It was created and has not been deleted.
    Active,
    /// This table is in the process of being imported with snapshot import.
    /// New documents may be created in this table by
    /// `ImportFacingModel::insert`. It may have the same name and/or number
    /// as an existing Active table. It appears in only one direction of
    /// TableMapping, so to find its mapping you must look it up by TableId,
    /// not TableNumber or TableName.
    Hidden,
    /// The table has been marked as deleted. Documents in the table may still
    /// exist, but they should be ignored.
    /// No new documents may be created in the table.
    /// A handful of legacy tables were not marked as Deleting but instead their
    /// _tables entry was deleted. Such tables should be treated the same as
    /// Deleting -- for now. Eventually we may want to clean up Deleting tables
    /// and delete the _tables rows or create a new Deleted state.
    Deleting,
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(any(test, feature = "testing"), derive(proptest_derive::Arbitrary))]
pub struct TableMetadata {
    pub name: TableName,
    pub number: TableNumber,
    pub state: TableState,
    // TODO(lee) allow any TableNamespace once they are supported in tests.
    #[cfg_attr(
        any(test, feature = "testing"),
        proptest(value = "TableNamespace::Global")
    )]
    pub namespace: TableNamespace,
    pub monotonic_creation_time: Option<bool>,
}

impl TableMetadata {
    pub fn is_active(&self) -> bool {
        matches!(self.state, TableState::Active)
    }
}

impl TableMetadata {
    pub fn new(namespace: TableNamespace, name: TableName, number: TableNumber) -> Self {
        Self {
            name,
            number,
            state: TableState::Active,
            namespace,
            monotonic_creation_time: None,
        }
    }

    pub fn new_with_state(
        namespace: TableNamespace,
        name: TableName,
        number: TableNumber,
        state: TableState,
    ) -> Self {
        Self {
            name,
            number,
            state,
            namespace,
            monotonic_creation_time: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct SerializedTableMetadata {
    name: String,
    number: i64,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<SerializedTableNamespace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monotonic_creation_time: Option<bool>,
}

impl TryFrom<TableMetadata> for SerializedTableMetadata {
    type Error = anyhow::Error;

    fn try_from(m: TableMetadata) -> anyhow::Result<Self> {
        Ok(Self {
            name: m.name.into(),
            number: u32::from(m.number) as i64,
            state: match m.state {
                TableState::Active => "active".to_owned(),
                TableState::Deleting => "deleting".to_owned(),
                TableState::Hidden => "hidden".to_owned(),
            },
            namespace: table_namespace_to_serialized(m.namespace)?,
            monotonic_creation_time: m.monotonic_creation_time,
        })
    }
}

impl TryFrom<SerializedTableMetadata> for TableMetadata {
    type Error = anyhow::Error;

    fn try_from(m: SerializedTableMetadata) -> anyhow::Result<Self> {
        Ok(Self {
            name: m.name.parse()?,
            number: u32::try_from(m.number)?.try_into()?,
            state: match &m.state[..] {
                "active" => TableState::Active,
                "deleting" => TableState::Deleting,
                "hidden" => TableState::Hidden,
                s => anyhow::bail!("invalid table state {s}"),
            },
            namespace: table_namespace_from_serialized(m.namespace)?,
            monotonic_creation_time: m.monotonic_creation_time,
        })
    }
}

codegen_convex_serialization!(TableMetadata, SerializedTableMetadata);

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "testing"), derive(proptest_derive::Arbitrary))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SerializedTableNamespace {
    ByComponent { id: String },
}

pub fn table_namespace_from_serialized(
    m: Option<SerializedTableNamespace>,
) -> anyhow::Result<TableNamespace> {
    Ok(match m {
        None => TableNamespace::Global,
        Some(SerializedTableNamespace::ByComponent { id }) => {
            TableNamespace::ByComponent(id.parse()?)
        },
    })
}

pub fn table_namespace_to_serialized(
    m: TableNamespace,
) -> anyhow::Result<Option<SerializedTableNamespace>> {
    match m {
        TableNamespace::Global => Ok(None),
        TableNamespace::ByComponent(id) => Ok(Some(SerializedTableNamespace::ByComponent {
            id: id.to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use value::{
        assert_obj,
        obj,
        ConvexObject,
        FieldName,
        TableNamespace,
    };

    use super::TableMetadata;
    use crate::bootstrap_model::tables::TableState;

    #[test]
    fn test_backwards_compatibility() -> anyhow::Result<()> {
        let serialized = obj!(
            "name" => "foo",
            "state" => "hidden",
            "number" => 1017,
        )?;
        let deserialized: TableMetadata = serialized.try_into().unwrap();
        assert_eq!(
            deserialized,
            TableMetadata {
                name: "foo".parse()?,
                number: 1017.try_into()?,
                state: TableState::Hidden,
                namespace: TableNamespace::Global,
                monotonic_creation_time: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_global_namespace() -> anyhow::Result<()> {
        let table = TableMetadata {
            name: "foo".parse()?,
            number: 1017.try_into()?,
            state: TableState::Active,
            namespace: TableNamespace::Global,
            monotonic_creation_time: None,
        };
        let serialized: ConvexObject = table.try_into()?;
        assert_eq!(
            serialized,
            assert_obj!(
                "name" => "foo",
                "state" => "active",
                "number" => 1017,
            ),
        );
        Ok(())
    }

    #[test]
    fn test_monotonic_creation_time_serialization() -> anyhow::Result<()> {
        // Test that monotonic_creation_time serializes when set
        let table = TableMetadata {
            name: "foo".parse()?,
            number: 1017.try_into()?,
            state: TableState::Active,
            namespace: TableNamespace::Global,
            monotonic_creation_time: Some(true),
        };
        let serialized: ConvexObject = table.try_into()?;
        // Check both camelCase and snake_case field names
        let field_name_camel: FieldName = "monotonicCreationTime".parse()?;
        let field_name_snake: FieldName = "monotonic_creation_time".parse()?;
        // The field should be present with one of these names
        let value = serialized
            .get(&field_name_camel)
            .or_else(|| serialized.get(&field_name_snake));
        assert_eq!(
            value,
            Some(&value::ConvexValue::Boolean(true)),
            "Serialized object: {:?}",
            serialized
        );

        // Test that it's omitted when None
        let table_no_flag = TableMetadata {
            name: "bar".parse()?,
            number: 1018.try_into()?,
            state: TableState::Active,
            namespace: TableNamespace::Global,
            monotonic_creation_time: None,
        };
        let serialized_no_flag: ConvexObject = table_no_flag.try_into()?;
        // The field should be absent when None
        assert_eq!(
            serialized_no_flag
                .get(&field_name_camel)
                .or_else(|| serialized_no_flag.get(&field_name_snake)),
            None
        );

        // Test deserialization - use snake_case to match SerializedTableMetadata
        let serialized_with_flag = obj!(
            "name" => "baz",
            "state" => "active",
            "number" => 1019,
            "monotonic_creation_time" => true,
        )?;
        let deserialized: TableMetadata = serialized_with_flag.try_into()?;
        assert_eq!(deserialized.monotonic_creation_time, Some(true));

        Ok(())
    }
}
