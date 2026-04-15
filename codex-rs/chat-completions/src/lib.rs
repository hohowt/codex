//! Translates Responses API requests into Chat Completions API requests
//! and converts the streaming response back into `ResponseEvent`s.

mod convert;
mod stream;

pub use convert::build_chat_request;
pub use stream::stream_chat_completions;
