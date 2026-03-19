//! Node subsystem for node-control.
//!
//! This module implements the node-control functionality for the gateway.
//! It includes the node registry, the WebSocket node, and the response handling.

pub mod node_registry;
pub mod response;
pub mod session_id;
pub mod ws_node;

pub use node_registry::{ConnectedNodeRegistry, NodeCommandResult};
pub use ws_node::handle_ws_node;
pub use response::handle_http_response;

