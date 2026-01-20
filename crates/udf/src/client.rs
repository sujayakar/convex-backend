use std::collections::BTreeMap;

use common::{
    bootstrap_model::components::definition::ComponentDefinitionMetadata,
    components::ComponentDefinitionPath,
    errors::JsError,
};
use pb::common::{
    function_result::Result as FunctionResultTypeProto,
    FunctionResult as FunctionResultProto,
};
use packed_value::PackedSyncValue;
use bytes::Bytes;

pub type EvaluateAppDefinitionsResult =
    BTreeMap<ComponentDefinitionPath, ComponentDefinitionMetadata>;

#[derive(Clone, Debug)]
#[cfg_attr(
    any(test, feature = "testing"),
    derive(proptest_derive::Arbitrary, PartialEq)
)]
pub struct FunctionResult {
    pub result: Result<PackedSyncValue, JsError>,
}

impl TryFrom<FunctionResultProto> for FunctionResult {
    type Error = anyhow::Error;

    fn try_from(result: FunctionResultProto) -> anyhow::Result<Self> {
        let result = match result.result {
            Some(FunctionResultTypeProto::PackedValue(value)) => {
                Ok(PackedSyncValue::from_bytes(Bytes::from(value))?)
            }
            Some(FunctionResultTypeProto::JsError(js_error)) => Err(js_error.try_into()?),
            None => anyhow::bail!("Missing result"),
        };
        Ok(FunctionResult { result })
    }
}

impl TryFrom<FunctionResult> for FunctionResultProto {
    type Error = anyhow::Error;

    fn try_from(result: FunctionResult) -> anyhow::Result<Self> {
        let result = match result.result {
            Ok(value) => FunctionResultTypeProto::PackedValue(value.as_bytes().to_vec()),
            Err(js_error) => FunctionResultTypeProto::JsError(js_error.try_into()?),
        };
        Ok(FunctionResultProto {
            result: Some(result),
        })
    }
}
