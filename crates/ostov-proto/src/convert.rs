//! Перевод между типами `ostov-api` и сгенерированными `pb`. Туда — без потерь (`From`),
//! обратно — с проверкой (`TryFrom`): с той стороны может прийти что угодно.

use std::collections::BTreeMap;

use ostov_api as api;
use ostov_api::{EngineError, ErrorCode};

use crate::pb;
use crate::pb::value::Kind;

type Result<T> = std::result::Result<T, EngineError>;

fn broken(what: &str) -> EngineError {
    EngineError::new(
        ErrorCode::Incompatible,
        format!("по сети пришли неверные данные: {what}"),
    )
}

// ---------------------------------------------------------------- значения

impl From<api::Value> for pb::Value {
    fn from(v: api::Value) -> Self {
        let kind = match v {
            api::Value::Null => Kind::Null(pb::Empty {}),
            api::Value::Int(n) => Kind::Int(n),
            api::Value::Text(s) => Kind::Text(s),
            api::Value::Decimal { units, scale } => Kind::Decimal(pb::Decimal {
                units,
                scale: u32::from(scale),
            }),
            api::Value::Bool(b) => Kind::Flag(b),
            api::Value::Date(d) => Kind::Date(d),
            api::Value::Ref { id, title } => Kind::Ref(pb::Ref { id, title }),
        };
        Self { kind: Some(kind) }
    }
}

impl TryFrom<pb::Value> for api::Value {
    type Error = EngineError;

    fn try_from(v: pb::Value) -> Result<Self> {
        Ok(match v.kind {
            None | Some(Kind::Null(_)) => Self::Null,
            Some(Kind::Int(n)) => Self::Int(n),
            Some(Kind::Text(s)) => Self::Text(s),
            Some(Kind::Decimal(d)) => Self::Decimal {
                units: d.units,
                scale: u8::try_from(d.scale).map_err(|_| broken("масштаб числа"))?,
            },
            Some(Kind::Flag(b)) => Self::Bool(b),
            Some(Kind::Date(d)) => Self::Date(d),
            Some(Kind::Ref(r)) => Self::Ref {
                id: r.id,
                title: r.title,
            },
        })
    }
}

pub fn fields_to_pb(fields: api::Fields) -> pb::Fields {
    pb::Fields {
        values: fields.into_iter().map(|(k, v)| (k, v.into())).collect(),
    }
}

pub fn fields_from_pb(fields: Option<pb::Fields>) -> Result<api::Fields> {
    fields
        .unwrap_or_default()
        .values
        .into_iter()
        .map(|(k, v)| Ok((k, v.try_into()?)))
        .collect()
}

// ---------------------------------------------------------------- ошибки

impl From<ErrorCode> for pb::ErrorCode {
    fn from(c: ErrorCode) -> Self {
        match c {
            ErrorCode::NotImplemented => Self::NotImplemented,
            ErrorCode::Unavailable => Self::Unavailable,
            ErrorCode::BadCredentials => Self::BadCredentials,
            ErrorCode::NeedFirstAdmin => Self::NeedFirstAdmin,
            ErrorCode::NeedFilePassword => Self::NeedFilePassword,
            ErrorCode::AlreadyOpen => Self::AlreadyOpen,
            ErrorCode::Forbidden => Self::Forbidden,
            ErrorCode::VersionConflict => Self::VersionConflict,
            ErrorCode::PeriodClosed => Self::PeriodClosed,
            ErrorCode::Validation => Self::Validation,
            ErrorCode::Incompatible => Self::Incompatible,
            ErrorCode::NotFound => Self::NotFound,
            ErrorCode::Internal => Self::Internal,
        }
    }
}

impl From<pb::ErrorCode> for ErrorCode {
    fn from(c: pb::ErrorCode) -> Self {
        match c {
            pb::ErrorCode::NotImplemented => Self::NotImplemented,
            pb::ErrorCode::Unavailable => Self::Unavailable,
            pb::ErrorCode::BadCredentials => Self::BadCredentials,
            pb::ErrorCode::NeedFirstAdmin => Self::NeedFirstAdmin,
            pb::ErrorCode::NeedFilePassword => Self::NeedFilePassword,
            pb::ErrorCode::AlreadyOpen => Self::AlreadyOpen,
            pb::ErrorCode::Forbidden => Self::Forbidden,
            pb::ErrorCode::VersionConflict => Self::VersionConflict,
            pb::ErrorCode::PeriodClosed => Self::PeriodClosed,
            pb::ErrorCode::Validation => Self::Validation,
            pb::ErrorCode::Incompatible => Self::Incompatible,
            pb::ErrorCode::NotFound => Self::NotFound,
            pb::ErrorCode::Unspecified | pb::ErrorCode::Internal => Self::Internal,
        }
    }
}

// ---------------------------------------------------------------- сессия

impl From<api::HelloInfo> for pb::HelloInfo {
    fn from(h: api::HelloInfo) -> Self {
        Self {
            protocol_version: h.protocol_version,
            config_name: h.config_name,
            config_version: h.config_version,
            config_format: h.config_format,
            login_required: h.login_required,
            first_run: h.first_run,
        }
    }
}

impl From<pb::HelloInfo> for api::HelloInfo {
    fn from(h: pb::HelloInfo) -> Self {
        Self {
            protocol_version: h.protocol_version,
            config_name: h.config_name,
            config_version: h.config_version,
            config_format: h.config_format,
            login_required: h.login_required,
            first_run: h.first_run,
        }
    }
}

impl pb::LoginReply {
    pub fn new(session: api::SessionInfo, token: String) -> Self {
        Self {
            login: session.login,
            display_name: session.display_name,
            roles: session.roles,
            token,
            system: Some(pb::SystemRights {
                closed_period: session.system.closed_period,
                set_closing_date: session.system.set_closing_date,
                admin: session.system.admin,
                update_config: session.system.update_config,
            }),
        }
    }
}

/// Сессия и её токен из ответа на вход.
pub fn session_from_pb(r: pb::LoginReply) -> (api::SessionInfo, String) {
    let session = api::SessionInfo {
        login: r.login,
        display_name: r.display_name,
        roles: r.roles,
        system: r
            .system
            .map(|s| api::SystemRights {
                closed_period: s.closed_period,
                set_closing_date: s.set_closing_date,
                admin: s.admin,
                update_config: s.update_config,
            })
            .unwrap_or_default(),
    };
    (session, r.token)
}

// ---------------------------------------------------------------- метаданные

impl From<api::FieldDef> for pb::FieldDef {
    fn from(f: api::FieldDef) -> Self {
        let (kind, scale) = match f.kind {
            api::FieldKind::Int => (pb::FieldType::Int, 0),
            api::FieldKind::Text => (pb::FieldType::Text, 0),
            api::FieldKind::Decimal { scale } => (pb::FieldType::Decimal, u32::from(scale)),
            api::FieldKind::Date => (pb::FieldType::Date, 0),
            api::FieldKind::Ref => (pb::FieldType::Ref, 0),
        };
        Self {
            name: f.name,
            title: f.title,
            r#type: kind.into(),
            scale,
            target: f.target,
            required: f.required,
            read_only: f.read_only,
        }
    }
}

impl TryFrom<pb::FieldDef> for api::FieldDef {
    type Error = EngineError;

    fn try_from(f: pb::FieldDef) -> Result<Self> {
        let kind = match pb::FieldType::try_from(f.r#type) {
            Ok(pb::FieldType::Int) => api::FieldKind::Int,
            Ok(pb::FieldType::Text) => api::FieldKind::Text,
            Ok(pb::FieldType::Decimal) => api::FieldKind::Decimal {
                scale: u8::try_from(f.scale).map_err(|_| broken("масштаб поля"))?,
            },
            Ok(pb::FieldType::Date) => api::FieldKind::Date,
            Ok(pb::FieldType::Ref) => api::FieldKind::Ref,
            Ok(pb::FieldType::Unspecified) | Err(_) => {
                return Err(broken(&format!("тип поля {}", f.name)));
            }
        };
        Ok(Self {
            name: f.name,
            title: f.title,
            kind,
            target: f.target,
            required: f.required,
            read_only: f.read_only,
        })
    }
}

fn defs_from_pb(fields: Vec<pb::FieldDef>) -> Result<Vec<api::FieldDef>> {
    fields.into_iter().map(TryInto::try_into).collect()
}

impl From<api::Rights> for pb::Rights {
    fn from(r: api::Rights) -> Self {
        Self {
            read: r.read,
            create: r.create,
            update: r.update,
            post: r.post,
            unpost: r.unpost,
            delete: r.delete,
        }
    }
}

impl From<pb::Rights> for api::Rights {
    fn from(r: pb::Rights) -> Self {
        Self {
            read: r.read,
            create: r.create,
            update: r.update,
            post: r.post,
            unpost: r.unpost,
            delete: r.delete,
        }
    }
}

impl From<api::MenuItem> for pb::MenuItem {
    fn from(m: api::MenuItem) -> Self {
        Self {
            title: m.title,
            icon: m.icon,
            object: m.object,
            children: m.children.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<pb::MenuItem> for api::MenuItem {
    fn from(m: pb::MenuItem) -> Self {
        Self {
            title: m.title,
            icon: m.icon,
            object: m.object,
            children: m.children.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<api::Meta> for pb::Meta {
    fn from(m: api::Meta) -> Self {
        Self {
            menu: m.menu.into_iter().map(Into::into).collect(),
            objects: m
                .objects
                .into_iter()
                .map(|o| pb::ObjectMeta {
                    name: o.name,
                    title: o.title,
                    kind: match o.kind {
                        api::ObjectKind::Catalog => pb::ObjectKind::Catalog,
                        api::ObjectKind::Document => pb::ObjectKind::Document,
                    }
                    .into(),
                    icon: o.icon,
                    fields: o.fields.into_iter().map(Into::into).collect(),
                    table_parts: o
                        .table_parts
                        .into_iter()
                        .map(|p| pb::TablePartDef {
                            name: p.name,
                            title: p.title,
                            fields: p.fields.into_iter().map(Into::into).collect(),
                        })
                        .collect(),
                    rights: Some(o.rights.into()),
                })
                .collect(),
            roles: m
                .roles
                .into_iter()
                .map(|r| pb::Role {
                    name: r.name,
                    title: r.title,
                })
                .collect(),
            registers: m
                .registers
                .into_iter()
                .map(|r| pb::RegisterMeta {
                    name: r.name,
                    title: r.title,
                    dimensions: r.dimensions.into_iter().map(Into::into).collect(),
                    resources: r.resources.into_iter().map(Into::into).collect(),
                })
                .collect(),
        }
    }
}

impl TryFrom<pb::Meta> for api::Meta {
    type Error = EngineError;

    fn try_from(m: pb::Meta) -> Result<Self> {
        let mut objects = Vec::with_capacity(m.objects.len());
        for o in m.objects {
            let kind = match pb::ObjectKind::try_from(o.kind) {
                Ok(pb::ObjectKind::Catalog) => api::ObjectKind::Catalog,
                Ok(pb::ObjectKind::Document) => api::ObjectKind::Document,
                Ok(pb::ObjectKind::Unspecified) | Err(_) => {
                    return Err(broken(&format!("вид объекта {}", o.name)));
                }
            };
            let mut table_parts = Vec::with_capacity(o.table_parts.len());
            for p in o.table_parts {
                table_parts.push(api::TablePartDef {
                    name: p.name,
                    title: p.title,
                    fields: defs_from_pb(p.fields)?,
                });
            }
            objects.push(api::ObjectMeta {
                name: o.name,
                title: o.title,
                kind,
                icon: o.icon,
                fields: defs_from_pb(o.fields)?,
                table_parts,
                rights: o.rights.map(Into::into).unwrap_or_default(),
            });
        }
        let mut registers = Vec::with_capacity(m.registers.len());
        for r in m.registers {
            registers.push(api::RegisterMeta {
                name: r.name,
                title: r.title,
                dimensions: defs_from_pb(r.dimensions)?,
                resources: defs_from_pb(r.resources)?,
            });
        }
        Ok(Self {
            menu: m.menu.into_iter().map(Into::into).collect(),
            objects,
            registers,
            roles: m
                .roles
                .into_iter()
                .map(|r| api::RoleInfo {
                    name: r.name,
                    title: r.title,
                })
                .collect(),
        })
    }
}

// ---------------------------------------------------------------- списки и объекты

impl From<api::ListQuery> for pb::ListQuery {
    fn from(q: api::ListQuery) -> Self {
        Self {
            object: q.object,
            order: q.order,
            descending: q.descending,
            search: q.search,
            posted: q.posted,
            deleted: q.deleted,
            after_key: q.after_key,
            size: q.size,
            filter: fields_to_pb(q.filter).values,
        }
    }
}

impl TryFrom<pb::ListQuery> for api::ListQuery {
    type Error = EngineError;

    fn try_from(q: pb::ListQuery) -> Result<Self> {
        Ok(Self {
            object: q.object,
            order: q.order,
            descending: q.descending,
            search: q.search,
            posted: q.posted,
            deleted: q.deleted,
            filter: fields_from_pb(Some(pb::Fields { values: q.filter }))?,
            after_key: q.after_key,
            size: q.size,
        })
    }
}

impl From<api::ListPage> for pb::ListPage {
    fn from(p: api::ListPage) -> Self {
        Self {
            rows: p
                .rows
                .into_iter()
                .map(|r| pb::Row {
                    id: r.id,
                    values: Some(fields_to_pb(r.values)),
                })
                .collect(),
            next_key: p.next_key,
            total: p.total,
            token: p.token,
        }
    }
}

impl TryFrom<pb::ListPage> for api::ListPage {
    type Error = EngineError;

    fn try_from(p: pb::ListPage) -> Result<Self> {
        Ok(Self {
            rows: p
                .rows
                .into_iter()
                .map(|r| {
                    Ok(api::Row {
                        id: r.id,
                        values: fields_from_pb(r.values)?,
                    })
                })
                .collect::<Result<_>>()?,
            next_key: p.next_key,
            total: p.total,
            token: p.token,
        })
    }
}

impl From<api::ObjectData> for pb::ObjectData {
    fn from(d: api::ObjectData) -> Self {
        Self {
            id: d.id,
            version: d.version,
            number: d.number,
            posted: d.posted,
            deleted: d.deleted,
            fields: Some(fields_to_pb(d.fields)),
            tables: d
                .tables
                .into_iter()
                .map(|(name, lines)| {
                    let lines = lines.into_iter().map(fields_to_pb).collect();
                    (name, pb::Lines { lines })
                })
                .collect(),
        }
    }
}

impl TryFrom<pb::ObjectData> for api::ObjectData {
    type Error = EngineError;

    fn try_from(d: pb::ObjectData) -> Result<Self> {
        let mut tables = BTreeMap::new();
        for (name, lines) in d.tables {
            let lines = lines
                .lines
                .into_iter()
                .map(|l| fields_from_pb(Some(l)))
                .collect::<Result<_>>()?;
            tables.insert(name, lines);
        }
        Ok(Self {
            id: d.id,
            version: d.version,
            number: d.number,
            posted: d.posted,
            deleted: d.deleted,
            fields: fields_from_pb(d.fields)?,
            tables,
        })
    }
}

impl From<api::PostAction> for pb::PostAction {
    fn from(p: api::PostAction) -> Self {
        match p {
            api::PostAction::Write => Self::Write,
            api::PostAction::Post => Self::Post,
            api::PostAction::Unpost => Self::Unpost,
        }
    }
}

/// Действие записи из поля `post` запроса: в prost перечисления приходят числом.
pub fn post_action_from_pb(p: i32) -> Result<api::PostAction> {
    match pb::PostAction::try_from(p) {
        Ok(pb::PostAction::Write) => Ok(api::PostAction::Write),
        Ok(pb::PostAction::Post) => Ok(api::PostAction::Post),
        Ok(pb::PostAction::Unpost) => Ok(api::PostAction::Unpost),
        Err(_) => Err(broken("действие записи")),
    }
}

impl From<api::Saved> for pb::Saved {
    fn from(s: api::Saved) -> Self {
        Self {
            id: s.id,
            version: s.version,
            number: s.number,
        }
    }
}

impl From<pb::Saved> for api::Saved {
    fn from(s: pb::Saved) -> Self {
        Self {
            id: s.id,
            version: s.version,
            number: s.number,
        }
    }
}

impl From<api::ReportQuery> for pb::ReportQuery {
    fn from(q: api::ReportQuery) -> Self {
        Self {
            report: q.report,
            params: Some(fields_to_pb(q.params)),
            after_key: q.after_key,
            size: q.size,
        }
    }
}

impl TryFrom<pb::ReportQuery> for api::ReportQuery {
    type Error = EngineError;

    fn try_from(q: pb::ReportQuery) -> Result<Self> {
        Ok(Self {
            report: q.report,
            params: fields_from_pb(q.params)?,
            after_key: q.after_key,
            size: q.size,
        })
    }
}

// ---------------------------------------------------------------- администрирование

impl From<api::UserInfo> for pb::UserInfo {
    fn from(u: api::UserInfo) -> Self {
        Self {
            login: u.login,
            display_name: u.display_name,
            role: u.role,
            active: u.active,
        }
    }
}

impl From<pb::UserInfo> for api::UserInfo {
    fn from(u: pb::UserInfo) -> Self {
        Self {
            login: u.login,
            display_name: u.display_name,
            role: u.role,
            active: u.active,
        }
    }
}

impl From<api::NewUser> for pb::NewUser {
    fn from(u: api::NewUser) -> Self {
        Self {
            login: u.login,
            display_name: u.display_name,
            role: u.role,
            password: u.password,
        }
    }
}

impl From<pb::NewUser> for api::NewUser {
    fn from(u: pb::NewUser) -> Self {
        Self {
            login: u.login,
            display_name: u.display_name,
            role: u.role,
            password: u.password,
        }
    }
}

impl From<api::JournalEntry> for pb::JournalEntry {
    fn from(e: api::JournalEntry) -> Self {
        Self {
            id: e.id,
            when: e.when,
            user: e.user,
            object: e.object,
            action: e.action,
            details: e.details,
        }
    }
}

impl From<pb::JournalEntry> for api::JournalEntry {
    fn from(e: pb::JournalEntry) -> Self {
        Self {
            id: e.id,
            when: e.when,
            user: e.user,
            object: e.object,
            action: e.action,
            details: e.details,
        }
    }
}
