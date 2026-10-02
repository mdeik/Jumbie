pub mod error;
pub mod query;
pub mod rpc;

// Behind a feature so Wasm plugin builds don't pull in the host-side RPC router's deps.
#[cfg(feature = "server")]
pub mod server;
pub mod traits;

// Convenience re-exports for the common types.
pub use error::PluginError;
pub use query::{SearchParams, SearchResponse, build_search_queries, extract_search_params};
pub use rpc::{ErrorResponse, JsonRpcRequest, JsonRpcResponse, ResultResponse};
#[cfg(feature = "server")]
pub use server::PluginServer;
pub use traits::{CallContext, PluginHandler, PluginTypeInfo};
