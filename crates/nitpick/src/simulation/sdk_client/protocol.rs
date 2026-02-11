//! Protocol types for SDK client ↔ Rust communication.
//!
//! WebSocket bridging reuses the same `JsOutgoingMessage` / `JsIncomingMessage`
//! types from the js_client module (connect/send/close/connected/message/closed).
//! This module adds types specific to the ConvexClient wrapper.

use serde::{
    Deserialize,
    Serialize,
};
use serde_json::Value as JsonValue;

/// Arguments for the `subscribe` JS function.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeArgs {
    pub query_name: String,
    pub args_json: String,
}

/// Result from `getSubscriptionResult` JS function.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionResult {
    pub value: Option<JsonValue>,
    pub error: Option<String>,
}

/// Arguments for the `runMutation` JS function.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMutationArgs {
    pub mutation_id: u32,
    pub name: String,
    pub args_json: String,
}

/// A completed mutation result from `getCompletedMutations`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompletedMutation {
    pub mutation_id: u32,
    pub result: MutationResult,
}

/// Mutation result (success or failure).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
pub enum MutationResult {
    #[serde(rename_all = "camelCase")]
    Success { value: JsonValue },

    #[serde(rename_all = "camelCase")]
    Failure { error: String },
}
