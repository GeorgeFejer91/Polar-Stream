use serde::Serialize;

pub(crate) type CommandResult<T> = Result<T, CommandError>;

/// Stable, renderer-safe error contract. Internal Rust/debug details stay out
/// of the WebView while the code remains suitable for programmatic handling.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

impl CommandError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}
