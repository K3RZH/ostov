//! gRPC-сервис: каждая команда — в вызов ядра той сессии, чей токен пришёл. Ядро синхронное,
//! поэтому вызов идёт в отдельном потоке (`spawn_blocking`) и не держит сетевой цикл.

use std::sync::Arc;

use ostov_api as api;
use ostov_api::{Engine, EngineResult};
use ostov_proto::pb::engine_server::Engine as Remote;
use ostov_proto::{AUTH_HEADER, fields_from_pb, fields_to_pb, pb, post_action_from_pb, to_status};
use tonic::{Request, Response, Status};

use crate::hub::Hub;

pub struct Service {
    pub(crate) hub: Arc<Hub>,
}

type Reply<T> = Result<Response<T>, Status>;

fn token<T>(req: &Request<T>) -> Option<String> {
    let value = req.metadata().get(AUTH_HEADER)?.to_str().ok()?;
    value.strip_prefix("Bearer ").map(str::to_owned)
}

async fn blocking<R: Send + 'static>(
    f: impl FnOnce() -> EngineResult<R> + Send + 'static,
) -> Reply<R> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Ok(Response::new(value)),
        Ok(Err(e)) => Err(to_status(&e)),
        Err(e) => Err(Status::internal(e.to_string())),
    }
}

impl Service {
    /// Команда от имени сессии из заголовка `authorization`.
    async fn with<T, R>(
        &self,
        req: Request<T>,
        f: impl FnOnce(&dyn Engine, T) -> EngineResult<R> + Send + 'static,
    ) -> Reply<R>
    where
        T: Send + 'static,
        R: Send + 'static,
    {
        let hub = Arc::clone(&self.hub);
        let token = token(&req);
        let body = req.into_inner();
        blocking(move || {
            let engine = hub.session(token.as_deref())?;
            f(engine.as_ref(), body)
        })
        .await
    }
}

#[tonic::async_trait]
impl Remote for Service {
    async fn hello(&self, _: Request<pb::Empty>) -> Reply<pb::HelloInfo> {
        let hub = Arc::clone(&self.hub);
        blocking(move || hub.hello().map(Into::into)).await
    }

    async fn login(&self, req: Request<pb::Credentials>) -> Reply<pb::LoginReply> {
        let hub = Arc::clone(&self.hub);
        let c = req.into_inner();
        blocking(move || {
            let (info, token) = hub.login(&c.login, &c.password)?;
            Ok(pb::LoginReply::new(info, token))
        })
        .await
    }

    async fn logout(&self, req: Request<pb::Empty>) -> Reply<pb::Empty> {
        let hub = Arc::clone(&self.hub);
        let token = token(&req);
        blocking(move || {
            if let Some(token) = token {
                hub.logout(&token);
            }
            Ok(pb::Empty {})
        })
        .await
    }

    async fn create_first_admin(&self, req: Request<pb::Credentials>) -> Reply<pb::LoginReply> {
        let hub = Arc::clone(&self.hub);
        let c = req.into_inner();
        blocking(move || {
            let (info, token) = hub.create_first_admin(&c.login, &c.password)?;
            Ok(pb::LoginReply::new(info, token))
        })
        .await
    }

    async fn get_meta(&self, req: Request<pb::Empty>) -> Reply<pb::Meta> {
        self.with(req, |e, _| Ok(e.get_meta()?.into())).await
    }

    async fn list(&self, req: Request<pb::ListQuery>) -> Reply<pb::ListPage> {
        self.with(req, |e, q| Ok(e.list(&q.into())?.into())).await
    }

    async fn seek(&self, req: Request<pb::SeekRequest>) -> Reply<pb::ListPage> {
        self.with(req, |e, s| Ok(e.seek(&s.token, s.position, s.size)?.into()))
            .await
    }

    async fn get(&self, req: Request<pb::ObjectRef>) -> Reply<pb::ObjectData> {
        self.with(req, |e, r| Ok(e.get(&r.object, r.id)?.into()))
            .await
    }

    async fn save(&self, req: Request<pb::SaveRequest>) -> Reply<pb::Saved> {
        self.with(req, |e, s| {
            let data = api::ObjectData::try_from(s.data.unwrap_or_default())?;
            let post = post_action_from_pb(s.post)?;
            Ok(e.save(&s.object, &data, post)?.into())
        })
        .await
    }

    async fn mark_deleted(&self, req: Request<pb::MarkDeletedRequest>) -> Reply<pb::Saved> {
        self.with(req, |e, m| {
            Ok(e.mark_deleted(&m.object, m.id, m.version, m.deleted)?
                .into())
        })
        .await
    }

    async fn post(&self, req: Request<pb::ObjectRef>) -> Reply<pb::Saved> {
        self.with(req, |e, r| Ok(e.post(&r.object, r.id)?.into()))
            .await
    }

    async fn unpost(&self, req: Request<pb::ObjectRef>) -> Reply<pb::Saved> {
        self.with(req, |e, r| Ok(e.unpost(&r.object, r.id)?.into()))
            .await
    }

    async fn run_report(&self, req: Request<pb::ReportQuery>) -> Reply<pb::ListPage> {
        self.with(req, |e, q| Ok(e.run_report(&q.try_into()?)?.into()))
            .await
    }

    async fn start_import(&self, req: Request<pb::JobRequest>) -> Reply<pb::JobId> {
        self.with(req, |e, j| {
            Ok(pb::JobId {
                id: e.start_import(&j.format, &j.path)?.0,
            })
        })
        .await
    }

    async fn start_export(&self, req: Request<pb::JobRequest>) -> Reply<pb::JobId> {
        self.with(req, |e, j| {
            Ok(pb::JobId {
                id: e.start_export(&j.format, &j.path)?.0,
            })
        })
        .await
    }

    async fn get_settings(&self, req: Request<pb::SettingsKey>) -> Reply<pb::Settings> {
        self.with(req, |e, k| {
            let values = e.get_settings(&k.key)?.map(fields_to_pb);
            Ok(pb::Settings { key: k.key, values })
        })
        .await
    }

    async fn save_settings(&self, req: Request<pb::Settings>) -> Reply<pb::Empty> {
        self.with(req, |e, s| {
            e.save_settings(&s.key, &fields_from_pb(s.values)?)?;
            Ok(pb::Empty {})
        })
        .await
    }

    async fn admin_users(&self, req: Request<pb::Empty>) -> Reply<pb::UserList> {
        self.with(req, |e, _| {
            let users = e.admin_users()?.into_iter().map(Into::into).collect();
            Ok(pb::UserList { users })
        })
        .await
    }

    async fn admin_add_user(&self, req: Request<pb::NewUser>) -> Reply<pb::Empty> {
        self.with(req, |e, u| {
            e.admin_add_user(&u.into())?;
            Ok(pb::Empty {})
        })
        .await
    }

    async fn admin_set_user_active(&self, req: Request<pb::UserActive>) -> Reply<pb::Empty> {
        let hub = Arc::clone(&self.hub);
        self.with(req, move |e, u| {
            e.admin_set_user_active(&u.login, u.active)?;
            if !u.active {
                hub.user_disabled(&u.login.trim().to_lowercase());
            }
            Ok(pb::Empty {})
        })
        .await
    }

    async fn admin_journal(&self, req: Request<pb::JournalRequest>) -> Reply<pb::JournalList> {
        self.with(req, |e, j| {
            let entries = e
                .admin_journal(j.after_key.as_deref(), j.size)?
                .into_iter()
                .map(Into::into)
                .collect();
            Ok(pb::JournalList { entries })
        })
        .await
    }

    async fn admin_backup(&self, req: Request<pb::BackupRequest>) -> Reply<pb::JobId> {
        self.with(req, |e, b| {
            Ok(pb::JobId {
                id: e.admin_backup(&b.target)?.0,
            })
        })
        .await
    }

    async fn get_closing_date(&self, req: Request<pb::Empty>) -> Reply<pb::ClosingDate> {
        self.with(req, |e, _| {
            Ok(pb::ClosingDate {
                date: e.closing_date()?,
            })
        })
        .await
    }

    async fn set_closing_date(&self, req: Request<pb::ClosingDate>) -> Reply<pb::Empty> {
        self.with(req, |e, c| {
            e.set_closing_date(c.date.as_deref())?;
            Ok(pb::Empty {})
        })
        .await
    }
}
