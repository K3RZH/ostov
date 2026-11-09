//! `Engine: Remote` (ADR-026): те же команды, что у локального ядра, но по gRPC поверх TLS.
//! Интерфейс знает только трейт `Engine`: в режиме 2 вместо `LocalConnector` он берёт
//! `RemoteConnector` и дальше работает так же.
//!
//! Команды синхронные, как весь `Engine`: внутри свой небольшой рантайм. Звать из обычного
//! (фонового) потока интерфейса, не из асинхронного кода.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ostov_api::{
    BatchItem, Connector, Engine, EngineError, EngineResult, ErrorCode, Fields, HelloInfo, JobId,
    JournalEntry, ListPage, ListQuery, Meta, NewAdmin, NewUser, ObjectData, PROTOCOL_VERSION,
    PostAction, ReportQuery, Saved, SessionInfo, UserInfo,
};
use ostov_proto::pb::engine_client::EngineClient;
use ostov_proto::{AUTH_HEADER, fields_from_pb, fields_to_pb, from_status, pb, session_from_pb};
use tokio::runtime::Runtime;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status};

/// Порт сервера, если в адресе он не указан.
pub const DEFAULT_PORT: u16 = 7700;

/// Подключается к серверу. `ca_pem` — его сертификат (`server.crt`): клиент доверяет
/// только ему, чужой сервер не примет (СТ-33).
#[derive(Clone)]
pub struct RemoteConnector {
    ca_pem: String,
}

impl RemoteConnector {
    pub fn new(ca_pem: impl Into<String>) -> Self {
        Self {
            ca_pem: ca_pem.into(),
        }
    }

    fn connect(&self, address: &str) -> EngineResult<RemoteEngine> {
        let (host, port) = split_address(address)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| unavailable(&e.to_string()))?;
        let tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&self.ca_pem))
            .domain_name(host.clone());
        let channel = runtime.block_on(async {
            Endpoint::from_shared(format!("https://{host}:{port}"))
                .map_err(|e| invalid_address(address, &e.to_string()))?
                .tls_config(tls)
                .map_err(|e| unavailable(&e.to_string()))?
                .connect()
                .await
                .map_err(|e| unavailable(&format!("{e:?}")))
        })?;
        let engine = RemoteEngine {
            client: EngineClient::new(channel),
            token: Mutex::new(None),
            runtime,
        };
        let server = engine.hello()?.protocol_version;
        if server != PROTOCOL_VERSION {
            return Err(EngineError::new(
                ErrorCode::Incompatible,
                format!(
                    "Версия протокола сервера {server}, а программы {PROTOCOL_VERSION}: обновите сервер и программу до одной версии."
                ),
            ));
        }
        Ok(engine)
    }
}

impl std::fmt::Debug for RemoteConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteConnector")
    }
}

impl Connector for RemoteConnector {
    /// `address` — `хост` или `хост:порт`, например `server.local:7700`.
    fn connect_remote(&self, address: &str) -> EngineResult<Arc<dyn Engine>> {
        let engine = self.connect(address)?;
        Ok(Arc::new(engine))
    }

    /// Первый запуск сервера: подключиться и создать администратора (СТ-36).
    /// Сервер обслуживает одну базу, поэтому `database` пока не используется.
    fn create_remote(
        &self,
        address: &str,
        _database: &str,
        admin: &NewAdmin,
    ) -> EngineResult<Arc<dyn Engine>> {
        let engine = self.connect(address)?;
        engine.create_first_admin(&admin.login, &admin.password)?;
        Ok(Arc::new(engine))
    }
}

pub struct RemoteEngine {
    client: EngineClient<Channel>,
    /// Токен сессии после входа; уходит в каждой команде.
    token: Mutex<Option<String>>,
    runtime: Runtime,
}

impl RemoteEngine {
    fn token(&self) -> MutexGuard<'_, Option<String>> {
        self.token.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Команда с токеном сессии; ответ сервера или его ошибка как `EngineError`.
    fn call<T, R, F>(
        &self,
        message: T,
        send: impl FnOnce(EngineClient<Channel>, Request<T>) -> F,
    ) -> EngineResult<R>
    where
        F: Future<Output = Result<tonic::Response<R>, Status>>,
    {
        let mut request = Request::new(message);
        if let Some(token) = self.token().as_deref()
            && let Ok(value) = format!("Bearer {token}").parse()
        {
            request.metadata_mut().insert(AUTH_HEADER, value);
        }
        let client = self.client.clone();
        self.runtime
            .block_on(send(client, request))
            .map(tonic::Response::into_inner)
            .map_err(|s| from_status(&s))
    }

    fn enter(&self, reply: pb::LoginReply) -> SessionInfo {
        let (session, token) = session_from_pb(reply);
        *self.token() = Some(token);
        session
    }
}

impl std::fmt::Debug for RemoteEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteEngine")
            .field("logged_in", &self.token().is_some())
            .finish()
    }
}

fn credentials(login: &str, password: &str) -> pb::Credentials {
    pb::Credentials {
        login: login.to_owned(),
        password: password.to_owned(),
    }
}

impl Engine for RemoteEngine {
    fn hello(&self) -> EngineResult<HelloInfo> {
        Ok(self
            .call(pb::Empty {}, |mut c, r| async move { c.hello(r).await })?
            .into())
    }

    fn login(&self, login: &str, password: &str) -> EngineResult<SessionInfo> {
        let reply = self.call(credentials(login, password), |mut c, r| async move {
            c.login(r).await
        })?;
        Ok(self.enter(reply))
    }

    fn logout(&self) -> EngineResult<()> {
        self.call(pb::Empty {}, |mut c, r| async move { c.logout(r).await })?;
        *self.token() = None;
        Ok(())
    }

    fn create_first_admin(&self, login: &str, password: &str) -> EngineResult<SessionInfo> {
        let reply = self.call(credentials(login, password), |mut c, r| async move {
            c.create_first_admin(r).await
        })?;
        Ok(self.enter(reply))
    }

    fn get_meta(&self) -> EngineResult<Meta> {
        self.call(pb::Empty {}, |mut c, r| async move { c.get_meta(r).await })?
            .try_into()
    }

    fn list(&self, query: &ListQuery) -> EngineResult<ListPage> {
        let q = pb::ListQuery::from(query.clone());
        self.call(q, |mut c, r| async move { c.list(r).await })?
            .try_into()
    }

    fn seek(&self, token: &str, position: u64, size: u32) -> EngineResult<ListPage> {
        let s = pb::SeekRequest {
            token: token.to_owned(),
            position,
            size,
        };
        self.call(s, |mut c, r| async move { c.seek(r).await })?
            .try_into()
    }

    fn get(&self, object: &str, id: i64) -> EngineResult<ObjectData> {
        self.call(
            object_ref(object, id),
            |mut c, r| async move { c.get(r).await },
        )?
        .try_into()
    }

    fn save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<Saved> {
        let s = pb::SaveRequest {
            object: object.to_owned(),
            data: Some(data.clone().into()),
            post: pb::PostAction::from(post).into(),
        };
        Ok(self
            .call(s, |mut c, r| async move { c.save(r).await })?
            .into())
    }

    fn save_batch(&self, items: &[BatchItem]) -> EngineResult<Vec<Saved>> {
        let b = pb::SaveBatchRequest {
            items: items
                .iter()
                .map(|i| pb::SaveRequest {
                    object: i.object.clone(),
                    data: Some(i.data.clone().into()),
                    post: pb::PostAction::from(i.post).into(),
                })
                .collect(),
        };
        let list = self.call(b, |mut c, r| async move { c.save_batch(r).await })?;
        Ok(list.saved.into_iter().map(Into::into).collect())
    }

    fn mark_deleted(
        &self,
        object: &str,
        id: i64,
        version: i64,
        deleted: bool,
    ) -> EngineResult<Saved> {
        let m = pb::MarkDeletedRequest {
            object: object.to_owned(),
            id,
            version,
            deleted,
        };
        Ok(self
            .call(m, |mut c, r| async move { c.mark_deleted(r).await })?
            .into())
    }

    fn post(&self, document: &str, id: i64) -> EngineResult<Saved> {
        Ok(self
            .call(object_ref(document, id), |mut c, r| async move {
                c.post(r).await
            })?
            .into())
    }

    fn unpost(&self, document: &str, id: i64) -> EngineResult<Saved> {
        Ok(self
            .call(object_ref(document, id), |mut c, r| async move {
                c.unpost(r).await
            })?
            .into())
    }

    fn run_report(&self, query: &ReportQuery) -> EngineResult<ListPage> {
        let q = pb::ReportQuery::from(query.clone());
        self.call(q, |mut c, r| async move { c.run_report(r).await })?
            .try_into()
    }

    fn start_import(&self, format: &str, source: &str) -> EngineResult<JobId> {
        let j = job(format, source);
        let id = self.call(j, |mut c, r| async move { c.start_import(r).await })?;
        Ok(JobId(id.id))
    }

    fn start_export(&self, format: &str, target: &str) -> EngineResult<JobId> {
        let j = job(format, target);
        let id = self.call(j, |mut c, r| async move { c.start_export(r).await })?;
        Ok(JobId(id.id))
    }

    fn get_settings(&self, key: &str) -> EngineResult<Option<Fields>> {
        let k = pb::SettingsKey {
            key: key.to_owned(),
        };
        let settings = self.call(k, |mut c, r| async move { c.get_settings(r).await })?;
        match settings.values {
            Some(values) => Ok(Some(fields_from_pb(Some(values))?)),
            None => Ok(None),
        }
    }

    fn save_settings(&self, key: &str, settings: &Fields) -> EngineResult<()> {
        let s = pb::Settings {
            key: key.to_owned(),
            values: Some(fields_to_pb(settings.clone())),
        };
        self.call(s, |mut c, r| async move { c.save_settings(r).await })?;
        Ok(())
    }

    fn admin_users(&self) -> EngineResult<Vec<UserInfo>> {
        let list = self.call(
            pb::Empty {},
            |mut c, r| async move { c.admin_users(r).await },
        )?;
        Ok(list.users.into_iter().map(Into::into).collect())
    }

    fn admin_add_user(&self, user: &NewUser) -> EngineResult<()> {
        let u = pb::NewUser::from(user.clone());
        self.call(u, |mut c, r| async move { c.admin_add_user(r).await })?;
        Ok(())
    }

    fn admin_set_user_active(&self, login: &str, active: bool) -> EngineResult<()> {
        let u = pb::UserActive {
            login: login.to_owned(),
            active,
        };
        self.call(
            u,
            |mut c, r| async move { c.admin_set_user_active(r).await },
        )?;
        Ok(())
    }

    fn admin_set_user_password(&self, login: &str, password: &str) -> EngineResult<()> {
        let u = pb::UserPassword {
            login: login.to_owned(),
            password: password.to_owned(),
        };
        self.call(
            u,
            |mut c, r| async move { c.admin_set_user_password(r).await },
        )?;
        Ok(())
    }

    fn admin_journal(&self, after_key: Option<&str>, size: u32) -> EngineResult<Vec<JournalEntry>> {
        let j = pb::JournalRequest {
            after_key: after_key.map(str::to_owned),
            size,
        };
        let list = self.call(j, |mut c, r| async move { c.admin_journal(r).await })?;
        Ok(list.entries.into_iter().map(Into::into).collect())
    }

    fn admin_backup(&self, target: &str) -> EngineResult<JobId> {
        let b = pb::BackupRequest {
            target: target.to_owned(),
        };
        let id = self.call(b, |mut c, r| async move { c.admin_backup(r).await })?;
        Ok(JobId(id.id))
    }

    fn closing_date(&self) -> EngineResult<Option<String>> {
        let d = self.call(pb::Empty {}, |mut c, r| async move {
            c.get_closing_date(r).await
        })?;
        Ok(d.date)
    }

    fn set_closing_date(&self, date: Option<&str>) -> EngineResult<()> {
        let d = pb::ClosingDate {
            date: date.map(str::to_owned),
        };
        self.call(d, |mut c, r| async move { c.set_closing_date(r).await })?;
        Ok(())
    }
}

fn object_ref(object: &str, id: i64) -> pb::ObjectRef {
    pb::ObjectRef {
        object: object.to_owned(),
        id,
    }
}

fn job(format: &str, path: &str) -> pb::JobRequest {
    pb::JobRequest {
        format: format.to_owned(),
        path: path.to_owned(),
    }
}

/// `хост` или `хост:порт` → хост и порт.
fn split_address(address: &str) -> EngineResult<(String, u16)> {
    let address = address.trim();
    match address.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            let port = port
                .parse()
                .map_err(|_| invalid_address(address, "порт не число"))?;
            Ok((host.to_owned(), port))
        }
        _ if !address.is_empty() => Ok((address.to_owned(), DEFAULT_PORT)),
        _ => Err(invalid_address(address, "адрес пустой")),
    }
}

fn invalid_address(address: &str, why: &str) -> EngineError {
    EngineError::new(
        ErrorCode::Validation,
        format!("Неверный адрес сервера «{address}»: {why}."),
    )
}

fn unavailable(details: &str) -> EngineError {
    EngineError {
        code: ErrorCode::Unavailable,
        message: "Нет связи с сервером. Проверьте адрес, сеть и что сервер запущен.".into(),
        details: details.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(
            split_address("server.local:7701").unwrap(),
            ("server.local".to_owned(), 7701)
        );
        assert_eq!(
            split_address(" server.local ").unwrap(),
            ("server.local".to_owned(), DEFAULT_PORT)
        );
        assert_eq!(
            split_address("server:порт").unwrap_err().code,
            ErrorCode::Validation
        );
        assert!(split_address("").is_err());
    }
}
