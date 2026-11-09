//! Сервер режима 2 (ADR-026): команды `Engine` по gRPC поверх TLS, сессии входа,
//! ядро на общей базе (PostgreSQL). Тонкая сетевая обёртка над `ostov-core`.

mod hub;
mod service;
mod tls;

use std::future::Future;
use std::sync::Arc;

use ostov_proto::pb::engine_server::EngineServer;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Identity, Server, ServerTlsConfig};

pub use hub::{Base, Connect, Hub};
pub use tls::{CERT_FILE, KEY_FILE, Tls};

/// Принимает клиентов на `listener` до сигнала `shutdown`. Без TLS не работает (СТ-33).
/// `Hub::start` зовите в рабочем потоке (`spawn_blocking`): он ходит в базу синхронно.
pub async fn serve(
    listener: TcpListener,
    hub: Arc<Hub>,
    tls: &Tls,
    shutdown: impl Future<Output = ()>,
) -> Result<(), tonic::transport::Error> {
    let identity = Identity::from_pem(&tls.cert_pem, &tls.key_pem);
    let service = service::Service {
        hub: Arc::clone(&hub),
    };
    let served = Server::builder()
        .tls_config(ServerTlsConfig::new().identity(identity))?
        .add_service(EngineServer::new(service))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown)
        .await;
    let _ = tokio::task::spawn_blocking(move || hub.close()).await;
    served
}
