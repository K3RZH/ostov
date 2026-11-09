//! Настоящий сервер на localhost по TLS и gRPC-клиент — так, как будет работать приложение.
//! База — файл SQLite; при заданном `OSTOV_PG_URL` основной сценарий идёт ещё и на PostgreSQL.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use ostov_api::{Connector, EngineError, ErrorCode};
use ostov_core::local::warehouse_model;
use ostov_proto::pb::engine_client::EngineClient;
use ostov_proto::{AUTH_HEADER, fields_to_pb, from_status, pb};
use ostov_server::{Base, Connect, Hub, Tls, serve};
use ostov_store::Driver;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tonic::Request;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

const ADMIN: (&str, &str) = ("boss", "admin-password-1");
const KEEPER: (&str, &str) = ("keeper", "keeper-password-1");

struct Running {
    client: EngineClient<Channel>,
    port: u16,
    dir: PathBuf,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn model() -> ostov_model::Config {
    let mut model = warehouse_model();
    let read = ostov_model::Rights {
        read: true,
        ..ostov_model::Rights::default()
    };
    model.roles.push(ostov_model::RoleDef {
        name: "storekeeper".into(),
        title: "Кладовщик".into(),
        objects: vec![("goods".into(), read)],
        system: ostov_model::SystemRights::default(),
    });
    model
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ostov-server-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sqlite(dir: &std::path::Path) -> Connect {
    let path = dir.join("base.db");
    Arc::new(move || {
        let db = ostov_store::drivers::sqlite::SqliteDriver::open(&path)?;
        Ok(Box::new(db) as Box<dyn Driver + Send>)
    })
}

/// Своя схема PostgreSQL на тест; `None`, если `OSTOV_PG_URL` не задан.
fn postgres(schema: &'static str) -> Option<Connect> {
    let url = std::env::var("OSTOV_PG_URL").ok()?;
    {
        let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url).unwrap();
        db.execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), &[])
            .unwrap();
        db.execute(&format!("CREATE SCHEMA {schema}"), &[]).unwrap();
    }
    Some(Arc::new(move || {
        let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url)?;
        db.execute(&format!("SET search_path TO {schema}"), &[])?;
        Ok(Box::new(db) as Box<dyn Driver + Send>)
    }))
}

async fn start(name: &str, connect: Option<Connect>) -> Running {
    start_in(temp(name), connect).await
}

/// Сервер в готовой папке: там может уже лежать `base.db`, созданная заранее.
async fn start_in(dir: PathBuf, connect: Option<Connect>) -> Running {
    let connect = connect.unwrap_or_else(|| sqlite(&dir));
    let hub = tokio::task::spawn_blocking(move || Hub::start(Base::new(connect, model())))
        .await
        .unwrap()
        .unwrap();
    let hub = Arc::new(hub);
    let tls = Tls::load_or_create(&dir.join("tls"), &["localhost".to_owned()]).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = oneshot::channel::<()>();
    let server_tls = tls.clone();
    tokio::spawn(async move {
        serve(listener, hub, &server_tls, async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    let client = EngineClient::new(connect_tls(port, &tls.cert_pem).await.unwrap());
    Running {
        client,
        port,
        dir,
        stop: Some(stop),
    }
}

async fn connect_tls(port: u16, ca_pem: &str) -> Result<Channel, tonic::transport::Error> {
    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca_pem))
        .domain_name("localhost");
    Endpoint::from_shared(format!("https://localhost:{port}"))?
        .tls_config(tls)?
        .connect()
        .await
}

fn authed<T>(msg: T, token: &str) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut()
        .insert(AUTH_HEADER, format!("Bearer {token}").parse().unwrap());
    req
}

fn creds((login, password): (&str, &str)) -> pb::Credentials {
    pb::Credentials {
        login: login.into(),
        password: password.into(),
    }
}

fn goods(name: &str) -> pb::SaveRequest {
    let fields = [("name".to_owned(), ostov_api::Value::Text(name.into()))].into();
    pb::SaveRequest {
        object: "goods".into(),
        data: Some(pb::ObjectData {
            fields: Some(fields_to_pb(fields)),
            ..pb::ObjectData::default()
        }),
        post: pb::PostAction::Write.into(),
    }
}

fn list(object: &str) -> pb::ListQuery {
    pb::ListQuery {
        object: object.into(),
        size: 100,
        ..pb::ListQuery::default()
    }
}

fn error<T: std::fmt::Debug>(r: Result<T, tonic::Status>) -> EngineError {
    from_status(&r.unwrap_err())
}

async fn first_run_then_login_and_work(connect: Option<Connect>, name: &str) {
    let mut s = start(name, connect).await;
    let c = &mut s.client;

    let hello = c.hello(pb::Empty {}).await.unwrap().into_inner();
    assert!(hello.first_run && hello.login_required);
    assert_eq!(hello.config_name, "Склад");
    assert_eq!(
        error(c.list(list("goods")).await).code,
        ErrorCode::NeedFirstAdmin
    );

    let admin = c
        .create_first_admin(creds(ADMIN))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(admin.roles, ["admin"]);
    let token = admin.token;
    c.save(authed(goods("Мука"), &token)).await.unwrap();
    let page = c
        .list(authed(list("goods"), &token))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page.rows.len(), 1);
    assert!(!c.hello(pb::Empty {}).await.unwrap().into_inner().first_run);

    // Без токена, с чужим токеном, с неверным паролем — нельзя.
    assert_eq!(
        error(c.list(list("goods")).await).code,
        ErrorCode::BadCredentials
    );
    assert_eq!(
        error(c.list(authed(list("goods"), "чужой")).await).code,
        ErrorCode::BadCredentials
    );
    assert_eq!(
        error(c.login(creds(("boss", "wrong-password-1"))).await).code,
        ErrorCode::BadCredentials
    );

    // Второй вход — своя сессия; выход закрывает только её.
    let second = c.login(creds(ADMIN)).await.unwrap().into_inner().token;
    assert_ne!(second, token);
    c.logout(authed(pb::Empty {}, &token)).await.unwrap();
    let gone = error(c.list(authed(list("goods"), &token)).await);
    assert_eq!(gone.code, ErrorCode::BadCredentials);
    assert!(gone.message.contains("войдите заново"), "{}", gone.message);
    c.list(authed(list("goods"), &second)).await.unwrap();
}

#[tokio::test]
async fn first_run_then_login_and_work_sqlite() {
    first_run_then_login_and_work(None, "flow").await;
}

#[tokio::test]
async fn first_run_then_login_and_work_postgres() {
    let made = tokio::task::spawn_blocking(|| postgres("ostov_t_server"));
    let Some(connect) = made.await.unwrap() else {
        return;
    };
    first_run_then_login_and_work(Some(connect), "flow-pg").await;
}

#[tokio::test]
async fn two_users_at_once_each_with_own_rights() {
    let mut s = start("two-users", None).await;
    let c = &mut s.client;
    let boss = c
        .create_first_admin(creds(ADMIN))
        .await
        .unwrap()
        .into_inner()
        .token;
    c.admin_add_user(authed(
        pb::NewUser {
            login: KEEPER.0.into(),
            display_name: "Кладовщик".into(),
            role: "storekeeper".into(),
            password: KEEPER.1.into(),
        },
        &boss,
    ))
    .await
    .unwrap();
    let keeper = c.login(creds(KEEPER)).await.unwrap().into_inner().token;

    // Одновременно: администратору можно, кладовщику — только смотреть.
    c.save(authed(goods("Сахар"), &boss)).await.unwrap();
    let denied = error(c.save(authed(goods("Соль"), &keeper)).await);
    assert_eq!(denied.code, ErrorCode::Forbidden);
    assert_eq!(denied.message, "Нет права на создание «Товары».");
    let page = c
        .list(authed(list("goods"), &keeper))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page.rows.len(), 1);
    let meta = c
        .get_meta(authed(pb::Empty {}, &keeper))
        .await
        .unwrap()
        .into_inner();
    let names: Vec<&str> = meta.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["goods"]);

    // Смена пароля: кладовщику нельзя; его сессия закрывается, свою администратор не теряет.
    let password = |login: &str, password: &str| pb::UserPassword {
        login: login.into(),
        password: password.into(),
    };
    let denied = error(
        c.admin_set_user_password(authed(password("boss", "keeper-took-it-1"), &keeper))
            .await,
    );
    assert_eq!(denied.code, ErrorCode::Forbidden);
    c.admin_set_user_password(authed(password("keeper", "keeper-password-2"), &boss))
        .await
        .unwrap();
    assert_eq!(
        error(c.list(authed(list("goods"), &keeper)).await).code,
        ErrorCode::BadCredentials
    );
    assert_eq!(
        error(c.login(creds(KEEPER)).await).code,
        ErrorCode::BadCredentials
    );
    let keeper = c
        .login(creds(("keeper", "keeper-password-2")))
        .await
        .unwrap()
        .into_inner()
        .token;
    c.admin_set_user_password(authed(password("boss", "admin-password-2"), &boss))
        .await
        .unwrap();
    c.list(authed(list("goods"), &boss)).await.unwrap();
    c.list(authed(list("goods"), &keeper)).await.unwrap();

    // Отключённый выходит сразу, а администратор работает дальше.
    c.admin_set_user_active(authed(
        pb::UserActive {
            login: "keeper".into(),
            active: false,
        },
        &boss,
    ))
    .await
    .unwrap();
    assert_eq!(
        error(c.list(authed(list("goods"), &keeper)).await).code,
        ErrorCode::BadCredentials
    );
    c.list(authed(list("goods"), &boss)).await.unwrap();
    let journal = c
        .admin_journal(authed(
            pb::JournalRequest {
                after_key: None,
                size: 100,
            },
            &boss,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(
        journal
            .entries
            .iter()
            .any(|e| e.action == "Отключение пользователя")
    );
}

#[tokio::test]
async fn core_errors_cross_the_wire_intact() {
    let mut s = start("errors", None).await;
    let c = &mut s.client;
    let token = c
        .create_first_admin(creds(ADMIN))
        .await
        .unwrap()
        .into_inner()
        .token;
    let id = |saved: pb::Saved| saved.id;
    let flour = id(c
        .save(authed(goods("Мука"), &token))
        .await
        .unwrap()
        .into_inner());
    let mut main = goods("Основной");
    main.object = "warehouses".into();
    let main = id(c.save(authed(main, &token)).await.unwrap().into_inner());

    let refer = |id| ostov_api::Value::Ref { id, title: None };
    let issue = pb::SaveRequest {
        object: "issue".into(),
        data: Some(pb::ObjectData {
            fields: Some(fields_to_pb(
                [
                    (
                        "date".to_owned(),
                        ostov_api::Value::Date("2026-02-01".into()),
                    ),
                    ("warehouse".to_owned(), refer(main)),
                ]
                .into(),
            )),
            tables: [(
                "lines".to_owned(),
                pb::Lines {
                    lines: vec![fields_to_pb(
                        [
                            ("goods".to_owned(), refer(flour)),
                            (
                                "qty".to_owned(),
                                ostov_api::Value::Decimal {
                                    units: 1_000,
                                    scale: 3,
                                },
                            ),
                        ]
                        .into(),
                    )],
                },
            )]
            .into(),
            ..pb::ObjectData::default()
        }),
        post: pb::PostAction::Post.into(),
    };
    let err = error(c.save(authed(issue, &token)).await);
    assert_eq!(err.code, ErrorCode::Validation);
    assert_eq!(
        err.message,
        "Не хватает остатка на 01.02.2026: Склад «Основной», Товар «Мука» — Количество стало бы −1,000. Списывают в этот день: этот документ."
    );
}

#[tokio::test]
async fn client_must_trust_the_server_certificate() {
    let s = start("tls", None).await;
    let stranger = temp("tls-stranger");
    let other = Tls::load_or_create(&stranger, &["localhost".to_owned()]).unwrap();
    // Чужой сертификат: соединение не устанавливается.
    let channel = connect_tls(s.port, &other.cert_pem).await;
    let failed = match channel {
        Err(_) => true,
        Ok(ch) => EngineClient::new(ch).hello(pb::Empty {}).await.is_err(),
    };
    assert!(failed);
    let _ = std::fs::remove_dir_all(stranger);

    // Сертификат сохраняется и при следующем запуске тот же.
    let again = Tls::load_or_create(&s.dir.join("tls"), &["localhost".to_owned()]).unwrap();
    let first = std::fs::read_to_string(s.dir.join("tls").join(ostov_server::CERT_FILE)).unwrap();
    assert_eq!(again.cert_pem, first);
}

/// ADR-033: база с одного компьютера, где пароли заданы по мягкой политике, на сервере.
#[tokio::test]
async fn weak_passwords_from_local_base_do_not_pass_server() {
    let dir = temp("weak");
    let path = dir.join("base.db").to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || {
        let local = ostov_core::LocalConnector::new(model())
            .with_password_policy(ostov_core::PasswordPolicy::LOCAL);
        let engine = local.create_local(&path, None, None).unwrap();
        engine.create_first_admin(ADMIN.0, ADMIN.1).unwrap();
        for (login, password) in [("keeper", "1234"), ("cook", "")] {
            engine
                .admin_add_user(&ostov_api::NewUser {
                    login: login.into(),
                    display_name: "Кладовщик".into(),
                    role: "storekeeper".into(),
                    password: password.into(),
                })
                .unwrap();
        }
    })
    .await
    .unwrap();

    let mut s = start_in(dir, None).await;
    let c = &mut s.client;
    for weak in [("keeper", "1234"), ("cook", "")] {
        let err = error(c.login(creds(weak)).await);
        assert_eq!(err.code, ErrorCode::Forbidden, "{weak:?}");
        assert!(err.message.contains("сменить"), "{}", err.message);
    }
    let boss = c.login(creds(ADMIN)).await.unwrap().into_inner().token;
    let password = |login: &str, password: &str| pb::UserPassword {
        login: login.into(),
        password: password.into(),
    };
    // На сервере новый пароль тоже только от 12 символов.
    let short = error(
        c.admin_set_user_password(authed(password("keeper", "12345"), &boss))
            .await,
    );
    assert_eq!(short.code, ErrorCode::Validation);
    c.admin_set_user_password(authed(password("keeper", KEEPER.1), &boss))
        .await
        .unwrap();
    let keeper = c.login(creds(KEEPER)).await.unwrap().into_inner().token;
    c.list(authed(list("goods"), &keeper)).await.unwrap();
    // Новый пользователь на сервере с коротким или пустым паролем не создаётся.
    for weak in ["", "1234"] {
        let err = error(
            c.admin_add_user(authed(
                pb::NewUser {
                    login: "temp".into(),
                    display_name: "Временный".into(),
                    role: "storekeeper".into(),
                    password: weak.into(),
                },
                &boss,
            ))
            .await,
        );
        assert_eq!(err.code, ErrorCode::Validation);
    }
}

/// У единственного администратора слабый пароль из режима 1: войти некому, помогает
/// только сброс из консоли сервера.
#[tokio::test]
async fn console_reset_unlocks_server_with_weak_admin() {
    let dir = temp("reset");
    let path = dir.join("base.db").to_string_lossy().into_owned();
    let base_dir = dir.clone();
    tokio::task::spawn_blocking(move || {
        let local = ostov_core::LocalConnector::new(model())
            .with_password_policy(ostov_core::PasswordPolicy::LOCAL);
        let engine = local.create_local(&path, None, None).unwrap();
        engine.create_first_admin(ADMIN.0, "").unwrap();
        drop(engine);
        let base = Base::new(sqlite(&base_dir), model());
        let short = base.reset_password(ADMIN.0, "1234").unwrap_err();
        assert_eq!(short.code, ErrorCode::Validation);
        let missing = base.reset_password("nobody", ADMIN.1).unwrap_err();
        assert_eq!(missing.code, ErrorCode::NotFound);
        base.reset_password(ADMIN.0, ADMIN.1).unwrap();
    })
    .await
    .unwrap();

    let mut s = start_in(dir, None).await;
    let c = &mut s.client;
    assert_eq!(
        error(c.login(creds((ADMIN.0, ""))).await).code,
        ErrorCode::BadCredentials
    );
    let boss = c.login(creds(ADMIN)).await.unwrap().into_inner().token;
    let journal = c
        .admin_journal(authed(
            pb::JournalRequest {
                after_key: None,
                size: 100,
            },
            &boss,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(
        journal
            .entries
            .iter()
            .any(|e| e.action == "Смена пароля" && e.user == "консоль сервера")
    );
}

#[tokio::test]
async fn console_reset_does_not_create_empty_base() {
    let dir = temp("reset-empty");
    let base_dir = dir.clone();
    let err = tokio::task::spawn_blocking(move || {
        Base::new(sqlite(&base_dir), model())
            .reset_password(ADMIN.0, ADMIN.1)
            .unwrap_err()
    })
    .await
    .unwrap();
    assert_eq!(err.code, ErrorCode::NotFound);
    let tables = {
        let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(dir.join("base.db")).unwrap();
        db.query("SELECT name FROM sqlite_master WHERE type = 'table'", &[])
            .unwrap()
    };
    assert!(tables.is_empty(), "{tables:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
