//! Shared helpers for JS UDF scenarios.

use application::{
    deploy_config::StartPushRequest,
    test_helpers::ApplicationTestExt,
    Application,
};
use common::{
    components::{
        CanonicalizedComponentFunctionPath,
        ComponentPath,
        PublicFunctionPath,
    },
    types::FunctionCaller,
    RequestId,
};
use keybroker::Identity;
use runtime::testing::TestRuntime;
use serde_json::Value as JsonValue;

/// Load the pre-built simulation package's StartPushRequest.
/// This contains all UDFs from npm-packages/simulation/convex/.
pub fn load_simulation_push() -> anyhow::Result<StartPushRequest> {
    let json = include_str!("../../../../npm-packages/simulation/dist/start_push.json");
    Ok(serde_json::from_str(json)?)
}

/// Parse a UDF path like "counter:increment" into a PublicFunctionPath.
pub fn udf_path(name: &str) -> anyhow::Result<PublicFunctionPath> {
    Ok(PublicFunctionPath::Component(
        CanonicalizedComponentFunctionPath {
            component: ComponentPath::test_user(),
            udf_path: name.parse()?,
        },
    ))
}

/// Deploy the simulation JS package to the application.
pub async fn deploy_js(application: &Application<TestRuntime>) -> anyhow::Result<()> {
    let push_request = load_simulation_push()?;
    application.run_test_push(push_request).await?;
    Ok(())
}

/// Call a mutation UDF and return the result as a JsonValue.
pub async fn call_mutation(
    application: &Application<TestRuntime>,
    path: &str,
    args: Vec<JsonValue>,
) -> anyhow::Result<JsonValue> {
    let result = application
        .mutation_udf(
            RequestId::new(),
            udf_path(path)?,
            args,
            Identity::system(),
            None,
            FunctionCaller::Test,
            None,
        )
        .await??;
    Ok(JsonValue::from(result.value.unpack()?))
}

/// Call a query UDF and return the result as a JsonValue.
pub async fn call_query(
    application: &Application<TestRuntime>,
    path: &str,
    args: Vec<JsonValue>,
) -> anyhow::Result<JsonValue> {
    let result = application
        .read_only_udf(
            RequestId::new(),
            udf_path(path)?,
            args,
            Identity::system(),
            FunctionCaller::Test,
        )
        .await?;
    Ok(JsonValue::from(result.result?.unpack()?))
}
