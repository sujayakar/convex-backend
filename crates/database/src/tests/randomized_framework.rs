use std::fmt::Debug;

use async_trait::async_trait;
use runtime::testing::{
    TestDriver,
    TestRuntime,
};

#[async_trait]
pub trait RandomizedScenario: Sized + Send {
    type Action: Send + Debug;

    async fn init(rt: TestRuntime) -> anyhow::Result<Self>;

    async fn apply_action(&mut self, action: Self::Action) -> anyhow::Result<()>;

    async fn check_invariants(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub fn run_randomized_actions<S>(actions: Vec<S::Action>)
where
    S: RandomizedScenario + 'static,
    S::Action: Send + 'static,
{
    let td = TestDriver::new();
    let rt = td.rt();
    let future = async move {
        let mut scenario = S::init(rt).await?;
        for action in actions {
            scenario.apply_action(action).await?;
            scenario.check_invariants().await?;
        }
        anyhow::Ok(())
    };
    td.run_until(future).unwrap();
}
