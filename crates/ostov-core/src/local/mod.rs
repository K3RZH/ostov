//! `LocalEngine` — реализация `ostov_api::Engine` поверх ядра в том же процессе (режим 1).
//! В режиме 2 его же будет вызывать сервер на каждую команду по gRPC.

mod config;
mod convert;
mod demo;
mod schema;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions, TryLockError};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ostov_api::{
    Connector, Engine, EngineError, EngineResult, ErrorCode, HelloInfo, ListPage, NewAdmin,
    ObjectData, PROTOCOL_VERSION, PostAction, ReportQuery, Row, Saved,
};
use ostov_store::drivers::sqlite::SqliteDriver;
use ostov_store::{Driver, Value};

pub use config::{Config, DocumentConfig, warehouse, warehouse_model};
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

impl LocalEngine {
    fn run_hello(&self) -> EngineResult<HelloInfo> {
        Ok(HelloInfo {
            protocol_version: PROTOCOL_VERSION,
            config_name: self.config.name.clone(),
            config_version: self.config.version.clone(),
            config_format: 0,
            login_required: false,
            first_run: false,
        })
    }

    fn run_list(&self, q: &ostov_api::ListQuery) -> EngineResult<ListPage> {
        let source = self.source(&q.object)?;
        let query = ListQuery {
            order: q.order.clone(),
            descending: q.descending,
            search: q.search.clone(),
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

    fn run_save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<Saved> {
        let mut fields = fields_from_api(&data.fields)?;
        let mut db = self.db();
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
                saved_doc(&doc.map_err(|e| self.explain(db, e))?)
            }
        };
        self.forget_lists();
        Ok(saved)
    }

    fn run_mark_deleted(
        &self,
        object: &str,
        id: i64,
        version: i64,
        deleted: bool,
    ) -> EngineResult<Saved> {
        let mut db = self.db();
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

    fn run_post(&self, document: &str, id: i64) -> EngineResult<Saved> {
        let cfg = self.document(document)?;
        let mut db = self.db();
        posting::post(&mut **db, &cfg.kind, &self.config.stock, id, cfg.rule)
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
        posting::unpost(&mut **db, &cfg.kind, &self.config.stock, id)
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

    fn list(&self, q: &ostov_api::ListQuery) -> EngineResult<ListPage> {
        self.guarded("List", || self.run_list(q))
    }

    fn seek(&self, token: &str, position: u64, size: u32) -> EngineResult<ListPage> {
        self.guarded("Seek", || self.run_seek(token, position, size))
    }

    fn get(&self, object: &str, id: i64) -> EngineResult<ObjectData> {
        self.guarded("Get", || self.run_get(object, id))
    }

    fn save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<Saved> {
        self.guarded("Save", || self.run_save(object, data, post))
    }

    fn mark_deleted(
        &self,
        object: &str,
        id: i64,
        version: i64,
        deleted: bool,
    ) -> EngineResult<Saved> {
        self.guarded("MarkDeleted", || {
            self.run_mark_deleted(object, id, version, deleted)
        })
    }

    fn post(&self, document: &str, id: i64) -> EngineResult<Saved> {
        self.guarded("Post", || self.run_post(document, id))
    }

    fn unpost(&self, document: &str, id: i64) -> EngineResult<Saved> {
        self.guarded("Unpost", || self.run_unpost(document, id))
    }

    fn run_report(&self, query: &ReportQuery) -> EngineResult<ListPage> {
        self.guarded("RunReport", || self.run_run_report(query))
    }

    fn get_meta(&self) -> EngineResult<ostov_api::Meta> {
        Ok(self.config.meta.clone())
    }

    fn closing_date(&self) -> EngineResult<Option<String>> {
        self.guarded("GetClosingDate", || {
            crate::closing::closing_date(&mut **self.db()).map_err(error)
        })
    }

    fn set_closing_date(&self, date: Option<&str>) -> EngineResult<()> {
        self.guarded("SetClosingDate", || {
            crate::closing::set_closing_date(&mut **self.db(), date).map_err(error)
        })
    }
}

/// Отчёты по остаткам:
/// - `stock_balance` — остатки на конец дня `date` (без даты — текущие);
/// - `stock_turnover` — обороты с `from` по `to`: `<ресурс>_opening`, `_receipt`, `_issue`, `_closing`.
///
/// Отбор — параметры с именами измерений (`warehouse`, `goods`). Строки окнами по `size`.
impl LocalEngine {
    fn run_run_report(&self, q: &ReportQuery) -> EngineResult<ListPage> {
        let stock = &self.config.stock;
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
            if stock.dims().iter().any(|d| &d.name == name) {
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
        let mut rows: Vec<ostov_api::Fields> = match q.report.as_str() {
            "stock_balance" => stock
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
            "stock_turnover" => stock
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
            other => {
                return Err(EngineError::new(
                    ErrorCode::NotFound,
                    format!("нет отчёта {other}"),
                ));
            }
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
        self.fill_titles(&mut **db, stock.dims(), &mut refs)?;

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
        let stock = &self.config.stock;
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

fn decimal_to_api(d: ostov_store::Decimal) -> ostov_api::Value {
    ostov_api::Value::Decimal {
        units: d.units(),
        scale: d.scale(),
    }
}

/// Открывает и создаёт локальные базы (режим 1).
///
/// Конфигурацию передаёт приложение: `LocalConnector::new(ostov_meta::load("config")?)`.
/// `LocalConnector::default()` — встроенный «Склад» из кода.
#[derive(Debug, Default, Clone)]
pub struct LocalConnector {
    model: Option<ostov_model::Config>,
}

impl LocalConnector {
    pub fn new(model: ostov_model::Config) -> Self {
        Self { model: Some(model) }
    }

    fn config(&self) -> EngineResult<Config> {
        match &self.model {
            Some(model) => Config::from_model(model),
            None => warehouse(),
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
        _admin: Option<&NewAdmin>,
    ) -> EngineResult<Arc<dyn Engine>> {
        no_file_password(file_password)?;
        let (db, config, lock) = create_base(path, self.config()?)?;
        Ok(Arc::new(
            LocalEngine::new(Box::new(db), config).locked(lock),
        ))
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
    in_transaction(&mut db, |db| {
        db.execute(
            &format!("CREATE TABLE {BASE_TABLE} (key TEXT PRIMARY KEY, value TEXT)"),
            &[],
        )?;
        db.execute(
            &format!("INSERT INTO {BASE_TABLE} (key, value) VALUES ('config', ?1)"),
            &[Value::Text(config.name.clone())],
        )?;
        config.create_tables(db)?;
        schema::record(db, &config)
    })
    .map_err(error)?;
    Ok((db, config, lock))
}

/// Доводит старую базу до текущей схемы. Сейчас одно: даты в движениях регистра
/// (их не было до отчётов по периодам) — берутся из поля `date` документов.
fn upgrade(db: &mut dyn Driver, config: &Config) -> Result<(), crate::CoreError> {
    crate::closing::create_table(db)?;
    if !schema::recorded(db)? {
        // Базы до реструктуризации создавались только под встроенный «Склад».
        schema::record(db, &warehouse()?)?;
    }
    schema::restructure(db, config)?;
    if config.stock.upgrade(db)? {
        for (table, doc) in &config.documents {
            if doc.kind.header().iter().any(|f| f.name == "date") {
                config.stock.fill_periods(db, table, "date")?;
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
