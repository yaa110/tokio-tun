/// Represents a general error type
pub type Error = Box<dyn std::error::Error>;

/// Represents an alias for standard library `Result` with error type of `Box<dyn Error>`.
pub type Result<T> = std::result::Result<T, Error>;
