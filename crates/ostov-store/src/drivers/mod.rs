#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sqlite")]
pub mod sqlite;

/// Тайм-аут запроса по умолчанию (СТ-51): дольше — `StoreError::Timeout`.
pub const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
