//! `LocalEngine` — реализация `ostov_api::Engine` поверх ядра в том же процессе (режим 1).
//! В режиме 2 его же будет вызывать сервер на каждую команду по gRPC.

mod access;
mod config;
mod convert;
mod demo;
mod journal;
mod schema;
mod settings;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions, TryLockError};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ostov_api::{
    BatchItem, Connector, Engine, EngineError, EngineResult, ErrorCode, HelloInfo, ListPage,
    NewAdmin, ObjectData, PROTOCOL_VERSION, PostAction, ReportQuery, Row, Saved,
};
use ostov_store::drivers::sqlite::SqliteDriver;
use ostov_store::{Driver, Value};

use access::Need;
use config::Parts;
pub use config::{BuiltinRules, Config, DocumentConfig, Rules, warehouse, warehouse_model};
use convert::{error, fields_from_api, fields_to_api, invalid};
use ostov_sec::Action;

use crate::closing::ClosedPeriodPass;

use crate::list::{MILESTONE_STEP, Milestones, TitleColumns, list_titled};
use crate::{
    Catalog, Document, Element, FieldDef, FieldKind, ListQuery, ListSource, in_transaction, posting,
};

pub struct LocalEngine {
    db: Mutex<Box<dyn Driver + Send>>,
    config: Config,
    /// Вехи по токену списка; сбрасываются при любой записи.
    milestones: Mutex<HashMap<String, Milestones>>,
    /// Справочник → его поле-название: по нему ссылки сортируются и ищутся.
    titles: TitleColumns,
    /// Кто вошёл и есть ли в базе пользователи.
    access: Mutex<access::Access>,
    /// Блокировка файла базы (СТ-55): пока движок жив, второй экземпляр базу не откроет.
    _lock: Option<File>,
}

enum Object<'a> {
    Catalog(&'a Catalog),
    Document(&'a DocumentConfig),
}

impl LocalEngine {
    pub fn new(mut db: Box<dyn Driver + Send>, config: Config) -> Self {
        let users = ostov_sec::has_users(&mut *db).unwrap_or(false);
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
            access: Mutex::new(access::Access { users, user: None }),
            _lock: None,
        }
    }

    /// Новая база Ostov в пустой базе данных любого драйвера (так её будет создавать и сервер).
    pub fn install(mut db: Box<dyn Driver + Send>, config: Config) -> EngineResult<Self> {
        in_transaction(&mut *db, |db| install(db, &config)).map_err(error)?;
        Ok(Self::new(db, config))
    }

    /// Существующая база: доводится до конфигурации (реструктуризация) и открывается.
    pub fn open(mut db: Box<dyn Driver + Send>, config: Config) -> EngineResult<Self> {
        in_transaction(&mut *db, |db| upgrade(db, &config)).map_err(error)?;
        Ok(Self::new(db, config))
    }

    fn locked(mut self, lock: File) -> Self {
        self._lock = Some(lock);
        self
    }

    // Если команда упала с паникой, транзакция уже откачена (см. `in_transaction`),
    // поэтому соединение можно брать и после «отравленного» замка.
    fn db(&self) -> MutexGuard<'_, Box<dyn Driver + Send>> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn milestones(&self) -> MutexGuard<'_, HashMap<String, Milestones>> {
        self.milestones
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Паника внутри команды становится ошибкой `Internal`, а не падением потока интерфейса.
    fn guarded<T>(&self, command: &str, f: impl FnOnce() -> EngineResult<T>) -> EngineResult<T> {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(result) => result,
            Err(payload) => {
                let why = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                // Вехи могли остаться недостроенными — проще построить заново.
                self.milestones().clear();
                Err(EngineError {
                    code: ErrorCode::Internal,
                    message: "Внутренняя ошибка ядра, действие отменено. Повторите; если не поможет — сообщите разработчикам.".into(),
                    details: format!("паника в команде {command}: {why}"),
                })
            }
        }
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
        self.milestones().clear();
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
        "{object}{SEP}{}{SEP}{}{SEP}{}{SEP}{}{SEP}{}",
        q.order.as_deref().unwrap_or(""),
        u8::from(q.descending),
        flag(q.posted),
        flag(q.deleted),
        q.search.as_deref().unwrap_or("")
    )
}

fn parse_token(token: &str) -> EngineResult<(String, ListQuery)> {
    let parts: Vec<&str> = token.splitn(6, SEP).collect();
    let [object, order, descending, posted, deleted, search] = parts.as_slice() else {
        return Err(invalid("испорченный токен списка".into()));
    };
    let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    Ok((
        (*object).to_owned(),
        ListQuery {
            order: some(order),
            descending: *descending == "1",
            search: some(search),
            posted: unflag(posted),
            deleted: unflag(deleted),
        },
    ))
}

fn flag(value: Option<bool>) -> &'static str {
    match value {
        None => "",
        Some(true) => "1",
        Some(false) => "0",
    }
}

fn unflag(s: &str) -> Option<bool> {
    match s {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
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

impl LocalEngine {
    fn run_hello(&self) -> EngineResult<HelloInfo> {
        Ok(HelloInfo {
            protocol_version: PROTOCOL_VERSION,
            config_name: self.config.name.clone(),
            config_version: self.config.version.clone(),
            config_format: 0,
            login_required: self.login_required(),
            first_run: false,
        })
    }

    fn run_list(&self, q: &ostov_api::ListQuery) -> EngineResult<ListPage> {
        let source = self.source(&q.object)?;
        let query = ListQuery {
            order: q.order.clone(),
            descending: q.descending,
            search: q.search.clone(),
            posted: q.posted,
            deleted: q.deleted,
        };
        let mut db = self.db();
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

    fn run_seek(&self, token: &str, position: u64, size: u32) -> EngineResult<ListPage> {
        let (object, query) = parse_token(token)?;
        self.need(&object, Need::Read)?;
        let source = self.source(&object)?;
        let mut db = self.db();
        let mut cache = self.milestones();
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

    fn run_get(&self, object: &str, id: i64) -> EngineResult<ObjectData> {
        let mut db = self.db();
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

    /// Перевод и пересчёт до записи: без базы, ошибки — по данным.
    fn prepare_save(
        &self,
        object: &str,
        data: &ObjectData,
        post: PostAction,
    ) -> EngineResult<(Object<'_>, crate::Fields, Parts)> {
        let mut fields = fields_from_api(&data.fields)?;
        let target = self.object(object)?;
        let mut parts = Parts::new();
        match &target {
            Object::Catalog(_) if post != PostAction::Write => {
                return Err(invalid(format!("{object} — справочник, он не проводится")));
            }
            Object::Catalog(_) => {}
            Object::Document(cfg) => {
                for (name, lines) in &data.tables {
                    let lines = lines
                        .iter()
                        .map(fields_from_api)
                        .collect::<Result<_, _>>()?;
                    parts.insert(name.clone(), lines);
                }
                cfg.rule.prepare(&mut fields, &mut parts).map_err(error)?;
            }
        }
        Ok((target, fields, parts))
    }

    fn run_save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<Saved> {
        let (target, fields, parts) = self.prepare_save(object, data, post)?;
        let mut guard = self.db();
        let db = &mut **guard;
        let saved = in_transaction(db, |db| {
            self.save_in(db, object, data, post, target, &fields, &parts)
        })
        .map_err(|e| self.explain(db, e))?;
        self.forget_lists();
        Ok(saved)
    }

    /// Пачка в одной транзакции: упала одна запись — не записалось ничего (импорт, Дизайн §12).
    fn run_save_batch(&self, items: &[BatchItem]) -> EngineResult<Vec<Saved>> {
        let mut prepared = Vec::with_capacity(items.len());
        for (n, item) in items.iter().enumerate() {
            let ready = self
                .prepare_save(&item.object, &item.data, item.post)
                .map_err(|e| numbered(n, e))?;
            prepared.push(ready);
        }
        let mut guard = self.db();
        let db = &mut **guard;
        let mut failed = 0;
        let saved = in_transaction(db, |db| {
            let mut saved = Vec::with_capacity(items.len());
            for (n, (item, (target, fields, parts))) in items.iter().zip(prepared).enumerate() {
                failed = n;
                saved.push(self.save_in(
                    db,
                    &item.object,
                    &item.data,
                    item.post,
                    target,
                    &fields,
                    &parts,
                )?);
            }
            Ok(saved)
        })
        .map_err(|e| numbered(failed, self.explain(db, e)))?;
        self.forget_lists();
        Ok(saved)
    }

    #[allow(clippy::too_many_arguments)]
    fn save_in(
        &self,
        db: &mut dyn Driver,
        object: &str,
        data: &ObjectData,
        post: PostAction,
        target: Object<'_>,
        fields: &crate::Fields,
        parts: &Parts,
    ) -> Result<Saved, crate::CoreError> {
        match target {
            Object::Catalog(c) => {
                let old = match data.id {
                    Some(id) => c.get(db, id)?.map(|e| e.fields),
                    None => None,
                };
                let element = match data.id {
                    None => c.insert(db, fields)?,
                    Some(id) => c.update(
                        db,
                        &Element {
                            id,
                            version: data.version,
                            deleted: data.deleted,
                            fields: fields.clone(),
                        },
                    )?,
                };
                let changes = self.changes(db, c.fields(), old.as_ref(), &element.fields);
                let action = if old.is_some() {
                    Action::Update
                } else {
                    Action::Create
                };
                if action == Action::Create || !changes.is_empty() {
                    self.journal(db, action, object, Some(element.id), &changes, "")?;
                }
                Ok(saved_element(&element))
            }
            Object::Document(cfg) => {
                let old = match data.id {
                    Some(id) => cfg.kind.get(db, id)?,
                    None => None,
                };
                let registers = self.config.registers_of(cfg);
                let rule = |doc: &Document| cfg.rule.movements(doc);
                let doc = match data.id {
                    None if post == PostAction::Post => {
                        posting::insert_and_post_to(db, &cfg.kind, &registers, fields, parts, rule)?
                    }
                    None => cfg.kind.insert(db, fields, parts)?,
                    Some(id) => {
                        let doc = Document {
                            id,
                            version: data.version,
                            number: data.number.unwrap_or_default(),
                            posted: data.posted,
                            deleted: data.deleted,
                            header: fields.clone(),
                            parts: parts.clone(),
                        };
                        match post {
                            PostAction::Write => cfg.kind.update(db, &doc)?,
                            PostAction::Post => {
                                posting::save_to(db, &cfg.kind, &registers, &doc, true, rule)?
                            }
                            PostAction::Unpost => {
                                posting::save_to(db, &cfg.kind, &registers, &doc, false, rule)?
                            }
                        }
                    }
                };
                let changes = self.changes(
                    db,
                    cfg.kind.header(),
                    old.as_ref().map(|d| &d.header),
                    &doc.header,
                );
                let lines_changed = old.as_ref().is_some_and(|d| d.parts != doc.parts);
                let note = if lines_changed {
                    let titles: Vec<String> = cfg
                        .kind
                        .parts()
                        .iter()
                        .filter(|p| {
                            old.as_ref().map(|d| d.parts.get(p.name()))
                                != Some(doc.parts.get(p.name()))
                        })
                        .map(|p| self.part_title(object, p.name()))
                        .collect();
                    format!("изменены строки: {}", titles.join(", "))
                } else {
                    String::new()
                };
                let action = if old.is_some() {
                    Action::Update
                } else {
                    Action::Create
                };
                if action == Action::Create || !changes.is_empty() || lines_changed {
                    self.journal(db, action, object, Some(doc.id), &changes, &note)?;
                }
                let was_posted = old.as_ref().is_some_and(|d| d.posted);
                match post {
                    PostAction::Post => {
                        self.journal(db, Action::Post, object, Some(doc.id), &[], "")?;
                    }
                    PostAction::Unpost if was_posted => {
                        self.journal(db, Action::Unpost, object, Some(doc.id), &[], "")?;
                    }
                    PostAction::Write | PostAction::Unpost => {}
                }
                Ok(saved_doc(&doc))
            }
        }
    }

    fn need_save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<()> {
        let need = if data.id.is_some() {
            Need::Update
        } else {
            Need::Create
        };
        self.need(object, need)?;
        match post {
            PostAction::Write => Ok(()),
            PostAction::Post => self.need(object, Need::Post),
            PostAction::Unpost => self.need(object, Need::Unpost),
        }
    }

    fn part_title(&self, object: &str, part: &str) -> String {
        self.config
            .meta
            .objects
            .iter()
            .find(|o| o.name == object)
            .and_then(|o| o.table_parts.iter().find(|p| p.name == part))
            .map_or_else(|| part.to_owned(), |p| p.title.clone())
    }

    fn run_mark_deleted(
        &self,
        object: &str,
        id: i64,
        version: i64,
        deleted: bool,
    ) -> EngineResult<Saved> {
        let target = self.object(object)?;
        let mut db = self.db();
        let action = if deleted {
            Action::MarkDeleted
        } else {
            Action::Unmark
        };
        let saved = in_transaction(&mut **db, |db| {
            let saved = match target {
                Object::Catalog(c) => saved_element(&c.mark_deleted(db, id, version, deleted)?),
                Object::Document(d) => saved_doc(&d.kind.mark_deleted(db, id, version, deleted)?),
            };
            self.journal(db, action, object, Some(id), &[], "")?;
            Ok(saved)
        })
        .map_err(error)?;
        self.forget_lists();
        Ok(saved)
    }

    fn run_post(&self, document: &str, id: i64) -> EngineResult<Saved> {
        let cfg = self.document(document)?;
        let mut db = self.db();
        let registers = self.config.registers_of(cfg);
        in_transaction(&mut **db, |db| {
            posting::post_to(db, &cfg.kind, &registers, id, |doc| cfg.rule.movements(doc))?;
            self.journal(db, Action::Post, document, Some(id), &[], "")
        })
        .map_err(|e| self.explain(&mut **db, e))?;
        let doc = cfg
            .kind
            .get(&mut **db, id)
            .map_err(error)?
            .ok_or_else(broken)?;
        self.forget_lists();
        Ok(saved_doc(&doc))
    }

    fn run_unpost(&self, document: &str, id: i64) -> EngineResult<Saved> {
        let cfg = self.document(document)?;
        let mut db = self.db();
        let registers = self.config.registers_of(cfg);
        in_transaction(&mut **db, |db| {
            posting::unpost_from(db, &cfg.kind, &registers, id)?;
            self.journal(db, Action::Unpost, document, Some(id), &[], "")
        })
        .map_err(|e| self.explain(&mut **db, e))?;
        let doc = cfg
            .kind
            .get(&mut **db, id)
            .map_err(error)?
            .ok_or_else(broken)?;
        self.forget_lists();
        Ok(saved_doc(&doc))
    }
}

impl Engine for LocalEngine {
    fn hello(&self) -> EngineResult<HelloInfo> {
        self.guarded("Hello", || self.run_hello())
    }

    fn login(&self, login: &str, password: &str) -> EngineResult<ostov_api::SessionInfo> {
        self.guarded("Login", || self.run_login(login, password))
    }

    fn logout(&self) -> EngineResult<()> {
        self.access().user = None;
        self.forget_lists();
        Ok(())
    }

    fn create_first_admin(
        &self,
        login: &str,
        password: &str,
    ) -> EngineResult<ostov_api::SessionInfo> {
        self.guarded("CreateFirstAdmin", || {
            self.run_create_first_admin(login, password)
        })
    }

    fn admin_users(&self) -> EngineResult<Vec<ostov_api::UserInfo>> {
        self.guarded("AdminUsers", || self.run_admin_users())
    }

    fn admin_add_user(&self, user: &ostov_api::NewUser) -> EngineResult<()> {
        self.guarded("AdminAddUser", || self.run_admin_add_user(user))
    }

    fn admin_journal(
        &self,
        after_key: Option<&str>,
        size: u32,
    ) -> EngineResult<Vec<ostov_api::JournalEntry>> {
        self.guarded("AdminJournal", || self.run_admin_journal(after_key, size))
    }

    fn admin_set_user_active(&self, login: &str, active: bool) -> EngineResult<()> {
        self.guarded("AdminSetUserActive", || {
            self.run_admin_set_user_active(login, active)
        })
    }

    fn list(&self, q: &ostov_api::ListQuery) -> EngineResult<ListPage> {
        self.guarded("List", || {
            self.need(&q.object, Need::Read)?;
            self.run_list(q)
        })
    }

    fn seek(&self, token: &str, position: u64, size: u32) -> EngineResult<ListPage> {
        self.guarded("Seek", || self.run_seek(token, position, size))
    }

    fn get(&self, object: &str, id: i64) -> EngineResult<ObjectData> {
        self.guarded("Get", || {
            self.need(object, Need::Read)?;
            self.run_get(object, id)
        })
    }

    fn save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<Saved> {
        self.guarded("Save", || {
            self.need_save(object, data, post)?;
            let _pass = ClosedPeriodPass::new(self.may_edit_closed());
            self.run_save(object, data, post)
        })
    }

    fn save_batch(&self, items: &[BatchItem]) -> EngineResult<Vec<Saved>> {
        self.guarded("SaveBatch", || {
            for (n, item) in items.iter().enumerate() {
                self.need_save(&item.object, &item.data, item.post)
                    .map_err(|e| numbered(n, e))?;
            }
            let _pass = ClosedPeriodPass::new(self.may_edit_closed());
            self.run_save_batch(items)
        })
    }

    fn mark_deleted(
        &self,
        object: &str,
        id: i64,
        version: i64,
        deleted: bool,
    ) -> EngineResult<Saved> {
        self.guarded("MarkDeleted", || {
            self.need(object, Need::Delete)?;
            let _pass = ClosedPeriodPass::new(self.may_edit_closed());
            self.run_mark_deleted(object, id, version, deleted)
        })
    }

    fn post(&self, document: &str, id: i64) -> EngineResult<Saved> {
        self.guarded("Post", || {
            self.need(document, Need::Post)?;
            let _pass = ClosedPeriodPass::new(self.may_edit_closed());
            self.run_post(document, id)
        })
    }

    fn unpost(&self, document: &str, id: i64) -> EngineResult<Saved> {
        self.guarded("Unpost", || {
            self.need(document, Need::Unpost)?;
            let _pass = ClosedPeriodPass::new(self.may_edit_closed());
            self.run_unpost(document, id)
        })
    }

    // Отчёты, настройки и дата закрытия — всем, кто вошёл: проверка входа в `need_system`.
    fn run_report(&self, query: &ReportQuery) -> EngineResult<ListPage> {
        self.guarded("RunReport", || {
            self.need_system(|_| true, "отчёты")?;
            self.run_run_report(query)
        })
    }

    fn get_settings(&self, key: &str) -> EngineResult<Option<ostov_api::Fields>> {
        self.guarded("GetSettings", || {
            self.need_system(|_| true, "настройки")?;
            settings::get(&mut **self.db(), key).map_err(error)
        })
    }

    fn save_settings(&self, key: &str, values: &ostov_api::Fields) -> EngineResult<()> {
        self.guarded("SaveSettings", || {
            self.need_system(|_| true, "настройки")?;
            settings::save(&mut **self.db(), key, values).map_err(error)
        })
    }

    fn get_meta(&self) -> EngineResult<ostov_api::Meta> {
        self.guarded("GetMeta", || self.run_get_meta())
    }

    fn closing_date(&self) -> EngineResult<Option<String>> {
        self.guarded("GetClosingDate", || {
            self.need_system(|_| true, "дата закрытия")?;
            crate::closing::closing_date(&mut **self.db()).map_err(error)
        })
    }

    fn set_closing_date(&self, date: Option<&str>) -> EngineResult<()> {
        self.guarded("SetClosingDate", || {
            self.need_system(|s| s.set_closing_date, "установка даты закрытия")?;
            let note = match date {
                Some(d) => format!("период закрыт по {}", ru_date(d)),
                None => "период открыт".to_owned(),
            };
            let mut db = self.db();
            in_transaction(&mut **db, |db| {
                crate::closing::set_closing_date(db, date)?;
                self.journal(db, Action::ClosingDateSet, "", None, &[], &note)
            })
            .map_err(error)
        })
    }
}

/// Отчёты по любому регистру накопления `<регистр>`:
/// - `<регистр>_balance` — остатки на конец дня `date` (без даты — текущие);
/// - `<регистр>_turnover` — обороты с `from` по `to`: `<ресурс>_opening`, `_receipt`, `_issue`, `_closing`.
///
/// Отбор — параметры с именами измерений (`warehouse`, `goods`). Строки окнами по `size`.
impl LocalEngine {
    fn run_run_report(&self, q: &ReportQuery) -> EngineResult<ListPage> {
        let no_report =
            || EngineError::new(ErrorCode::NotFound, format!("нет отчёта {}", q.report));
        let (register, kind) = q
            .report
            .rsplit_once('_')
            .and_then(|(name, kind)| Some((self.config.registers.get(name)?, kind)))
            .ok_or_else(no_report)?;
        let date = |name: &str| -> EngineResult<Option<String>> {
            match q.params.get(name) {
                None | Some(ostov_api::Value::Null) => Ok(None),
                Some(ostov_api::Value::Date(d)) => Ok(Some(d.clone())),
                Some(_) => Err(invalid(format!("параметр {name} должен быть датой"))),
            }
        };
        let allowed = ["date", "from", "to"];
        let mut filter = crate::Fields::new();
        for (name, value) in &q.params {
            if register.dims().iter().any(|d| &d.name == name) {
                if *value != ostov_api::Value::Null {
                    filter.insert(name.clone(), convert::value_from_api(value)?);
                }
            } else if !allowed.contains(&name.as_str()) {
                return Err(invalid(format!(
                    "у отчёта {} нет параметра {name}",
                    q.report
                )));
            }
        }

        let mut db = self.db();
        let mut rows: Vec<ostov_api::Fields> = match kind {
            "balance" => register
                .balances(&mut **db, date("date")?.as_deref(), &filter)
                .map_err(error)?
                .into_iter()
                .map(|(dims, amounts)| {
                    let mut row = fields_to_api(&dims);
                    for (name, value) in amounts {
                        row.insert(name, decimal_to_api(value));
                    }
                    row
                })
                .collect(),
            "turnover" => register
                .turnovers(
                    &mut **db,
                    date("from")?.as_deref(),
                    date("to")?.as_deref(),
                    &filter,
                )
                .map_err(error)?
                .into_iter()
                .map(|t| {
                    let mut row = fields_to_api(&t.dims);
                    for (part, amounts) in [
                        ("opening", t.opening),
                        ("receipt", t.receipt),
                        ("issue", t.issue),
                        ("closing", t.closing),
                    ] {
                        for (name, value) in amounts {
                            row.insert(format!("{name}_{part}"), decimal_to_api(value));
                        }
                    }
                    row
                })
                .collect(),
            _ => return Err(no_report()),
        };

        let total = rows.len();
        let start: usize = match q.after_key.as_deref() {
            None => 0,
            Some(key) => key
                .parse()
                .map_err(|_| invalid("испорченный ключ страницы отчёта".into()))?,
        };
        let size = window(q.size);
        if size == 0 || size > crate::list::MAX_WINDOW {
            return Err(invalid(format!(
                "размер окна должен быть от 1 до {}",
                crate::list::MAX_WINDOW
            )));
        }
        let end = start.saturating_add(size).min(total);
        let mut page: Vec<ostov_api::Fields> = rows.drain(start.min(total)..end).collect();
        let mut refs: Vec<&mut ostov_api::Fields> = page.iter_mut().collect();
        self.fill_titles(&mut **db, register.dims(), &mut refs)?;

        Ok(ListPage {
            rows: page
                .into_iter()
                .zip(start..)
                .map(|(values, n)| Row {
                    id: i64::try_from(n + 1).unwrap_or(i64::MAX),
                    values,
                })
                .collect(),
            next_key: (end < total).then(|| end.to_string()),
            total: u64::try_from(total).ok(),
            token: q.report.clone(),
        })
    }
}

impl LocalEngine {
    /// Нехватка остатка — человеческим языком: что, где, на какой день и какие документы
    /// списывают (вопрос интерфейса: «нет товара и документа, который мешает»).
    /// Транзакция уже откачена: новый документ в базе не найдётся — он «этот документ».
    fn explain(&self, db: &mut dyn Driver, e: crate::CoreError) -> EngineError {
        let crate::CoreError::NegativeBalance(shortage) = &e else {
            return error(e);
        };
        let crate::error::Shortage {
            resource,
            balance,
            date,
            dims,
            blockers,
            ..
        } = &**shortage;
        let Some(stock) = self.config.registers.get(&shortage.register) else {
            return error(e);
        };
        let title = |defs: &[FieldDef], name: &str| {
            defs.iter()
                .find(|f| f.name == name)
                .map_or_else(|| name.to_owned(), |f| f.title.clone())
        };
        let mut what = Vec::new();
        for (name, value) in dims {
            let def = stock.dims().iter().find(|f| &f.name == name);
            let shown = match (value, def.and_then(|f| f.target.as_deref())) {
                (Value::Int(id), Some(target)) => self
                    .config
                    .catalogs
                    .get(target)
                    .and_then(|c| c.titles(db, &[*id]).ok())
                    .and_then(|mut t| t.remove(id))
                    .unwrap_or_else(|| id.to_string()),
                (Value::Int(n), None) => n.to_string(),
                (Value::Text(s), _) => s.clone(),
                _ => "—".to_owned(),
            };
            what.push(format!("{} «{shown}»", title(stock.dims(), name)));
        }
        let mut message = format!(
            "Не хватает остатка{}: {} — {} стало бы {}.",
            date.as_deref()
                .map(|d| format!(" на {}", ru_date(d)))
                .unwrap_or_default(),
            what.join(", "),
            title(stock.resources(), resource),
            balance.to_string().replace('-', "−").replace('.', ","),
        );
        let docs: Vec<String> = blockers
            .iter()
            .map(|(table, id)| {
                let doc = self.config.meta.objects.iter().find(|o| &o.name == table);
                let kind = doc.map_or(table.as_str(), |o| o.title.as_str());
                match self
                    .config
                    .documents
                    .get(table)
                    .and_then(|d| d.kind.get(db, *id).ok().flatten())
                {
                    Some(found) => format!("{kind} №{}", found.number),
                    None => "этот документ".to_owned(),
                }
            })
            .collect();
        if !docs.is_empty() {
            message.push_str(&format!(" Списывают в этот день: {}.", docs.join(", ")));
        }
        EngineError {
            message,
            ..error(e)
        }
    }
}

/// `2026-01-31` → `31.01.2026`.
fn ru_date(iso: &str) -> String {
    let mut parts = iso.split('-').rev();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(d), Some(m), Some(y)) => format!("{d}.{m}.{y}"),
        _ => iso.to_owned(),
    }
}

/// Ошибка записи из пачки: человеку сразу видно, какая это запись по счёту.
fn numbered(n: usize, e: EngineError) -> EngineError {
    EngineError {
        message: format!("Запись {}: {}", n + 1, e.message),
        ..e
    }
}

fn decimal_to_api(d: ostov_store::Decimal) -> ostov_api::Value {
    ostov_api::Value::Decimal {
        units: d.units(),
        scale: d.scale(),
    }
}

/// Открывает и создаёт локальные базы (режим 1).
///
/// Конфигурацию передаёт приложение: `LocalConnector::new(ostov_meta::load("config")?)`.
/// `LocalConnector::default()` — встроенный «Склад» из кода. Правила документов — встроенные,
/// другие (Python) — через `with_rules`.
#[derive(Default, Clone)]
pub struct LocalConnector {
    model: Option<ostov_model::Config>,
    rules: Option<Arc<dyn Rules + Send + Sync>>,
}

impl std::fmt::Debug for LocalConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConnector")
            .field("model", &self.model.as_ref().map(|m| &m.name))
            .field("rules", &self.rules.is_some())
            .finish()
    }
}

impl LocalConnector {
    pub fn new(model: ostov_model::Config) -> Self {
        Self {
            model: Some(model),
            rules: None,
        }
    }

    /// Свои правила документов вместо встроенных, например из Python (`ostov-py`).
    pub fn with_rules(mut self, rules: Arc<dyn Rules + Send + Sync>) -> Self {
        self.rules = Some(rules);
        self
    }

    fn config(&self) -> EngineResult<Config> {
        let model = self.model.clone().unwrap_or_else(warehouse_model);
        match &self.rules {
            Some(rules) => Config::from_model_with(&model, rules.as_ref()),
            None => Config::from_model(&model),
        }
        .map_err(error)
    }
}

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
        let config = self.config()?;
        in_transaction(&mut db, |db| upgrade(db, &config)).map_err(error)?;
        Ok(Arc::new(
            LocalEngine::new(Box::new(db), config).locked(lock),
        ))
    }

    fn create_local(
        &self,
        path: &str,
        file_password: Option<&str>,
        admin: Option<&NewAdmin>,
    ) -> EngineResult<Arc<dyn Engine>> {
        no_file_password(file_password)?;
        let (db, config, lock) = create_base(path, self.config()?)?;
        let engine = LocalEngine::new(Box::new(db), config).locked(lock);
        if let Some(admin) = admin {
            engine.create_first_admin(&admin.login, &admin.password)?;
        }
        Ok(Arc::new(engine))
    }
}

/// Новый файл базы со служебной таблицей и таблицами конфигурации.
fn create_base(path: &str, config: Config) -> EngineResult<(SqliteDriver, Config, File)> {
    if Path::new(path).exists() {
        return Err(invalid(format!(
            "файл {path} уже есть: выберите другое имя"
        )));
    }
    let lock = lock_base(path)?;
    let mut db = SqliteDriver::open(path).map_err(|e| error(e.into()))?;
    in_transaction(&mut db, |db| install(db, &config)).map_err(error)?;
    Ok((db, config, lock))
}

/// Служебные таблицы и таблицы конфигурации в пустой базе.
fn install(db: &mut dyn Driver, config: &Config) -> Result<(), crate::CoreError> {
    let insert = format!(
        "INSERT INTO {BASE_TABLE} (key, value) VALUES ('config', {})",
        db.dialect().placeholder(1)
    );
    db.execute(
        &format!("CREATE TABLE {BASE_TABLE} (key TEXT PRIMARY KEY, value TEXT)"),
        &[],
    )?;
    db.execute(&insert, &[Value::Text(config.name.clone())])?;
    config.create_tables(db)?;
    settings::create_table(db)?;
    ostov_sec::create_tables(db)?;
    schema::record(db, config)
}

/// Доводит старую базу до текущей схемы. Сейчас одно: даты в движениях регистра
/// (их не было до отчётов по периодам) — берутся из поля `date` документов.
fn upgrade(db: &mut dyn Driver, config: &Config) -> Result<(), crate::CoreError> {
    crate::closing::create_table(db)?;
    settings::create_table(db)?;
    ostov_sec::create_tables(db)?;
    if !schema::recorded(db)? {
        // Базы до реструктуризации создавались только под встроенный «Склад».
        schema::record(db, &warehouse()?)?;
    }
    schema::restructure(db, config)?;
    for register in config.registers.values() {
        if register.upgrade(db)? {
            for (table, doc) in &config.documents {
                let moves = doc.registers.iter().any(|r| r == register.name());
                if moves && doc.kind.header().iter().any(|f| f.name == "date") {
                    register.fill_periods(db, table, "date")?;
                }
            }
        }
    }
    Ok(())
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
