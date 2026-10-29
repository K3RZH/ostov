//! `LocalEngine` — реализация `ostov_api::Engine` поверх ядра в том же процессе (режим 1).
//! В режиме 2 его же будет вызывать сервер на каждую команду по gRPC.

mod config;
mod convert;
mod demo;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use ostov_api::{
    Connector, Engine, EngineError, EngineResult, ErrorCode, HelloInfo, ListPage, NewAdmin,
    ObjectData, PROTOCOL_VERSION, PostAction, Row, Saved,
};
use ostov_store::drivers::sqlite::SqliteDriver;
use ostov_store::{Driver, Value};

pub use config::{Config, DocumentConfig, warehouse};
use convert::{error, fields_from_api, fields_to_api, invalid};

use crate::list::{MILESTONE_STEP, Milestones, TitleColumns, list_titled};
use crate::{
    Catalog, Document, Element, FieldDef, FieldKind, ListQuery, ListSource, in_transaction,
    insert_and_post, posting,
};

pub struct LocalEngine {
    db: Mutex<Box<dyn Driver + Send>>,
    config: Config,
    /// Вехи по токену списка; сбрасываются при любой записи.
    milestones: Mutex<HashMap<String, Milestones>>,
    /// Справочник → его поле-название: по нему ссылки сортируются и ищутся.
    titles: TitleColumns,
    /// Блокировка файла базы (СТ-55): пока движок жив, второй экземпляр базу не откроет.
    _lock: Option<File>,
}

enum Object<'a> {
    Catalog(&'a Catalog),
    Document(&'a DocumentConfig),
}

impl LocalEngine {
    pub fn new(db: Box<dyn Driver + Send>, config: Config) -> Self {
        let titles = config
            .catalogs
            .iter()
            .filter_map(|(table, c)| {
                let field = c.fields().iter().find(|f| f.kind == FieldKind::Text)?;
                Some((table.clone(), field.name.clone()))
            })
            .collect();
        Self {
            db: Mutex::new(db),
            titles,
            config,
            milestones: Mutex::new(HashMap::new()),
            _lock: None,
        }
    }

    fn locked(mut self, lock: File) -> Self {
        self._lock = Some(lock);
        self
    }

    fn db(&self) -> EngineResult<MutexGuard<'_, Box<dyn Driver + Send>>> {
        self.db.lock().map_err(|_| broken())
    }

    fn object(&self, name: &str) -> EngineResult<Object<'_>> {
        if let Some(c) = self.config.catalogs.get(name) {
            return Ok(Object::Catalog(c));
        }
        if let Some(d) = self.config.documents.get(name) {
            return Ok(Object::Document(d));
        }
        Err(EngineError::new(
            ErrorCode::NotFound,
            format!("в конфигурации нет объекта {name}"),
        ))
    }

    fn source(&self, name: &str) -> EngineResult<&dyn ListSource> {
        Ok(match self.object(name)? {
            Object::Catalog(c) => c,
            Object::Document(d) => &d.kind,
        })
    }

    fn document(&self, name: &str) -> EngineResult<&DocumentConfig> {
        match self.object(name)? {
            Object::Document(d) => Ok(d),
            Object::Catalog(_) => Err(invalid(format!("{name} — справочник, он не проводится"))),
        }
    }

    fn forget_lists(&self) {
        if let Ok(mut m) = self.milestones.lock() {
            m.clear();
        }
    }

    fn page(
        &self,
        db: &mut dyn Driver,
        source: &dyn ListSource,
        token: String,
        page: crate::ListPage,
    ) -> EngineResult<ListPage> {
        let mut rows: Vec<Row> = page
            .rows
            .into_iter()
            .map(|r| Row {
                id: r.id,
                values: fields_to_api(&r.fields),
            })
            .collect();
        let mut values: Vec<&mut ostov_api::Fields> =
            rows.iter_mut().map(|r| &mut r.values).collect();
        self.fill_titles(db, &source.list_fields(), &mut values)?;
        Ok(ListPage {
            rows,
            next_key: page.next_key,
            total: page.total,
            token,
        })
    }

    /// Подставляет названия в ссылки: один запрос на поле, а не на строку.
    fn fill_titles(
        &self,
        db: &mut dyn Driver,
        defs: &[FieldDef],
        rows: &mut [&mut ostov_api::Fields],
    ) -> EngineResult<()> {
        for def in defs.iter().filter(|d| d.kind == FieldKind::Ref) {
            let Some(catalog) = def
                .target
                .as_ref()
                .and_then(|t| self.config.catalogs.get(t))
            else {
                continue;
            };
            let mut ids: Vec<i64> = rows
                .iter()
                .filter_map(|r| match r.get(&def.name) {
                    Some(ostov_api::Value::Ref { id, .. }) => Some(*id),
                    _ => None,
                })
                .collect();
            ids.sort_unstable();
            ids.dedup();
            if ids.is_empty() {
                continue;
            }
            let titles = catalog.titles(db, &ids).map_err(error)?;
            for row in rows.iter_mut() {
                if let Some(ostov_api::Value::Ref { id, title }) = row.get_mut(&def.name) {
                    *title = titles.get(id).cloned();
                }
            }
        }
        Ok(())
    }
}

fn broken() -> EngineError {
    EngineError::new(
        ErrorCode::Internal,
        "Ядро остановилось после ошибки, откройте базу заново.",
    )
}

fn window(size: u32) -> usize {
    usize::try_from(size).unwrap_or(usize::MAX)
}

// Токен списка — объект, сортировка и поиск; по нему `seek` находит вехи.
const SEP: char = '\u{1f}';

fn token(object: &str, q: &ListQuery) -> String {
    format!(
        "{object}{SEP}{}{SEP}{}{SEP}{}",
        q.order.as_deref().unwrap_or(""),
        u8::from(q.descending),
        q.search.as_deref().unwrap_or("")
    )
}

fn parse_token(token: &str) -> EngineResult<(String, ListQuery)> {
    let parts: Vec<&str> = token.splitn(4, SEP).collect();
    let [object, order, descending, search] = parts.as_slice() else {
        return Err(invalid("испорченный токен списка".into()));
    };
    let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    Ok((
        (*object).to_owned(),
        ListQuery {
            order: some(order),
            descending: *descending == "1",
            search: some(search),
        },
    ))
}

fn saved_doc(d: &Document) -> Saved {
    Saved {
        id: d.id,
        version: d.version,
        number: Some(d.number),
    }
}

fn saved_element(e: &Element) -> Saved {
    Saved {
        id: e.id,
        version: e.version,
        number: None,
    }
}

impl Engine for LocalEngine {
    fn hello(&self) -> EngineResult<HelloInfo> {
        Ok(HelloInfo {
            protocol_version: PROTOCOL_VERSION,
            config_name: self.config.name.clone(),
            config_version: self.config.version.clone(),
            config_format: 0,
            login_required: false,
            first_run: false,
        })
    }

    fn list(&self, q: &ostov_api::ListQuery) -> EngineResult<ListPage> {
        let source = self.source(&q.object)?;
        let query = ListQuery {
            order: q.order.clone(),
            descending: q.descending,
            search: q.search.clone(),
        };
        let mut db = self.db()?;
        let page = list_titled(
            &mut **db,
            source,
            &query,
            q.after_key.as_deref(),
            window(q.size),
            &self.titles,
        )
        .map_err(error)?;
        self.page(&mut **db, source, token(&q.object, &query), page)
    }

    fn seek(&self, token: &str, position: u64, size: u32) -> EngineResult<ListPage> {
        let (object, query) = parse_token(token)?;
        let source = self.source(&object)?;
        let mut db = self.db()?;
        let mut cache = self.milestones.lock().map_err(|_| broken())?;
        if !cache.contains_key(token) {
            let built =
                Milestones::build_titled(&mut **db, source, &query, MILESTONE_STEP, &self.titles)
                    .map_err(error)?;
            cache.insert(token.to_owned(), built);
        }
        let marks = cache.get(token).ok_or_else(broken)?;
        let page = marks
            .seek(&mut **db, source, position, window(size))
            .map_err(error)?;
        drop(cache);
        self.page(&mut **db, source, token.to_owned(), page)
    }

    fn get(&self, object: &str, id: i64) -> EngineResult<ObjectData> {
        let mut db = self.db()?;
        let missing =
            || EngineError::new(ErrorCode::NotFound, format!("в {object} нет записи {id}"));
        match self.object(object)? {
            Object::Catalog(c) => {
                let e = c.get(&mut **db, id).map_err(error)?.ok_or_else(missing)?;
                let mut fields = fields_to_api(&e.fields);
                self.fill_titles(&mut **db, c.fields(), &mut [&mut fields])?;
                Ok(ObjectData {
                    id: Some(e.id),
                    version: e.version,
                    number: None,
                    posted: false,
                    deleted: e.deleted,
                    fields,
                    tables: BTreeMap::new(),
                })
            }
            Object::Document(cfg) => {
                let d = cfg
                    .kind
                    .get(&mut **db, id)
                    .map_err(error)?
                    .ok_or_else(missing)?;
                let mut fields = fields_to_api(&d.header);
                self.fill_titles(&mut **db, cfg.kind.header(), &mut [&mut fields])?;
                let mut tables = BTreeMap::new();
                for part in cfg.kind.parts() {
                    let mut lines: Vec<ostov_api::Fields> = d
                        .parts
                        .get(part.name())
                        .into_iter()
                        .flatten()
                        .map(fields_to_api)
                        .collect();
                    let mut refs: Vec<&mut ostov_api::Fields> = lines.iter_mut().collect();
                    self.fill_titles(&mut **db, part.fields(), &mut refs)?;
                    tables.insert(part.name().to_owned(), lines);
                }
                Ok(ObjectData {
                    id: Some(d.id),
                    version: d.version,
                    number: Some(d.number),
                    posted: d.posted,
                    deleted: d.deleted,
                    fields,
                    tables,
                })
            }
        }
    }

    fn save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<Saved> {
        let mut fields = fields_from_api(&data.fields)?;
        let mut db = self.db()?;
        let db = &mut **db;
        let saved = match self.object(object)? {
            Object::Catalog(c) => {
                if post != PostAction::Write {
                    return Err(invalid(format!("{object} — справочник, он не проводится")));
                }
                let element = match data.id {
                    None => c.insert(db, &fields),
                    Some(id) => c.update(
                        db,
                        &Element {
                            id,
                            version: data.version,
                            deleted: data.deleted,
                            fields,
                        },
                    ),
                };
                saved_element(&element.map_err(error)?)
            }
            Object::Document(cfg) => {
                let mut parts = data
                    .tables
                    .iter()
                    .map(|(name, lines)| {
                        let lines = lines
                            .iter()
                            .map(fields_from_api)
                            .collect::<Result<_, _>>()?;
                        Ok((name.clone(), lines))
                    })
                    .collect::<EngineResult<BTreeMap<_, _>>>()?;
                (cfg.prepare)(&mut fields, &mut parts).map_err(error)?;
                let stock = &self.config.stock;
                let doc = match data.id {
                    None if post == PostAction::Post => {
                        insert_and_post(db, &cfg.kind, stock, &fields, &parts, cfg.rule)
                    }
                    None => cfg.kind.insert(db, &fields, &parts),
                    Some(id) => {
                        let doc = Document {
                            id,
                            version: data.version,
                            number: data.number.unwrap_or_default(),
                            posted: data.posted,
                            deleted: data.deleted,
                            header: fields,
                            parts,
                        };
                        match post {
                            PostAction::Write => cfg.kind.update(db, &doc),
                            PostAction::Post => {
                                posting::save(db, &cfg.kind, stock, &doc, true, cfg.rule)
                            }
                            PostAction::Unpost => {
                                posting::save(db, &cfg.kind, stock, &doc, false, cfg.rule)
                            }
                        }
                    }
                };
                saved_doc(&doc.map_err(error)?)
            }
        };
        self.forget_lists();
        Ok(saved)
    }

    fn mark_deleted(
        &self,
        object: &str,
        id: i64,
        version: i64,
        deleted: bool,
    ) -> EngineResult<Saved> {
        let mut db = self.db()?;
        let saved = match self.object(object)? {
            Object::Catalog(c) => saved_element(
                &c.mark_deleted(&mut **db, id, version, deleted)
                    .map_err(error)?,
            ),
            Object::Document(d) => saved_doc(
                &d.kind
                    .mark_deleted(&mut **db, id, version, deleted)
                    .map_err(error)?,
            ),
        };
        self.forget_lists();
        Ok(saved)
    }

    fn post(&self, document: &str, id: i64) -> EngineResult<Saved> {
        let cfg = self.document(document)?;
        let mut db = self.db()?;
        posting::post(&mut **db, &cfg.kind, &self.config.stock, id, cfg.rule).map_err(error)?;
        let doc = cfg
            .kind
            .get(&mut **db, id)
            .map_err(error)?
            .ok_or_else(broken)?;
        self.forget_lists();
        Ok(saved_doc(&doc))
    }

    fn unpost(&self, document: &str, id: i64) -> EngineResult<Saved> {
        let cfg = self.document(document)?;
        let mut db = self.db()?;
        posting::unpost(&mut **db, &cfg.kind, &self.config.stock, id).map_err(error)?;
        let doc = cfg
            .kind
            .get(&mut **db, id)
            .map_err(error)?
            .ok_or_else(broken)?;
        self.forget_lists();
        Ok(saved_doc(&doc))
    }
}

/// Открывает и создаёт локальные базы (режим 1) с временной конфигурацией «Склад».
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalConnector;

// Служебная таблица: по ней видно, что файл — база Ostov.
const BASE_TABLE: &str = "ostov_base";

impl Connector for LocalConnector {
    fn open_local(&self, path: &str, file_password: Option<&str>) -> EngineResult<Arc<dyn Engine>> {
        no_file_password(file_password)?;
        if !Path::new(path).exists() {
            return Err(EngineError::new(
                ErrorCode::NotFound,
                format!("файла базы {path} нет"),
            ));
        }
        let lock = lock_base(path)?;
        let mut db = SqliteDriver::open(path).map_err(|e| error(e.into()))?;
        let sql = format!("SELECT value FROM {BASE_TABLE} WHERE key = 'config'");
        if db.query(&sql, &[]).is_err() {
            return Err(EngineError::new(
                ErrorCode::Incompatible,
                format!("{path} — не база Ostov"),
            ));
        }
        let config = warehouse().map_err(error)?;
        Ok(Arc::new(
            LocalEngine::new(Box::new(db), config).locked(lock),
        ))
    }

    fn create_local(
        &self,
        path: &str,
        file_password: Option<&str>,
        _admin: Option<&NewAdmin>,
    ) -> EngineResult<Arc<dyn Engine>> {
        no_file_password(file_password)?;
        let (db, config, lock) = create_base(path)?;
        Ok(Arc::new(
            LocalEngine::new(Box::new(db), config).locked(lock),
        ))
    }
}

/// Новый файл базы со служебной таблицей и таблицами конфигурации.
fn create_base(path: &str) -> EngineResult<(SqliteDriver, Config, File)> {
    if Path::new(path).exists() {
        return Err(invalid(format!(
            "файл {path} уже есть: выберите другое имя"
        )));
    }
    let lock = lock_base(path)?;
    let config = warehouse().map_err(error)?;
    let mut db = SqliteDriver::open(path).map_err(|e| error(e.into()))?;
    in_transaction(&mut db, |db| {
        db.execute(
            &format!("CREATE TABLE {BASE_TABLE} (key TEXT PRIMARY KEY, value TEXT)"),
            &[],
        )?;
        db.execute(
            &format!("INSERT INTO {BASE_TABLE} (key, value) VALUES ('config', ?1)"),
            &[Value::Text(config.name.clone())],
        )?;
        config.create_tables(db)
    })
    .map_err(error)?;
    Ok((db, config, lock))
}

/// Системная блокировка `<база>.lock`: снимается сама, даже если программа упала.
fn lock_base(path: &str) -> EngineResult<File> {
    let lock_path = format!("{path}.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| EngineError {
            code: ErrorCode::Unavailable,
            message: format!("не удалось открыть {lock_path}"),
            details: e.to_string(),
        })?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(EngineError::new(
            ErrorCode::AlreadyOpen,
            "База уже открыта в другом окне или программе.",
        )),
        Err(TryLockError::Error(e)) => Err(EngineError {
            code: ErrorCode::Unavailable,
            message: "не удалось заблокировать файл базы".into(),
            details: e.to_string(),
        }),
    }
}

fn no_file_password(password: Option<&str>) -> EngineResult<()> {
    match password {
        Some(_) => Err(invalid(
            "пароль на файл базы пока не поддерживается (вопрос В-2)".into(),
        )),
        None => Ok(()),
    }
}
