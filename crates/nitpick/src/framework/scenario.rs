use std::fmt::Debug;

use application::Application;
use async_trait::async_trait;
use futures::future::BoxFuture;
use runtime::testing::TestRuntime;

/// A scenario to run under the DST framework.
///
/// Each scenario defines:
/// - How to set up the application (deploy JS, initialize state)
/// - How to run a single transaction
/// - How to validate invariants during and after execution
#[async_trait]
pub trait Scenario: Send + Sync + 'static {
    type TestRun: TestRun;

    fn name(&self) -> &'static str;

    /// Set up the scenario: deploy UDFs via `run_test_push`, initialize state.
    /// The Application has already been created but has no deployed code.
    async fn start_run(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<Self::TestRun>;
}

/// An instance of a running scenario.
#[async_trait]
pub trait TestRun: Send + 'static {
    /// Output describing the final system state. Must be deterministic and
    /// comparable for determinism verification.
    type Output: Debug + Eq + Send;

    /// Run a single transaction of the scenario. The runner will ignore
    /// OCC errors but fail the test on any other error.
    fn run_transaction(
        &self,
        rt: TestRuntime,
        application: &Application<TestRuntime>,
    ) -> BoxFuture<'static, anyhow::Result<()>>;

    /// Validate the scenario's invariants at the current state.
    /// Called probabilistically during execution and always at the end.
    async fn validate(
        &self,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<()>;

    /// Finalize the test and return an output for determinism comparison.
    async fn finalize(
        &self,
        application: &Application<TestRuntime>,
    ) -> anyhow::Result<Self::Output>;
}
