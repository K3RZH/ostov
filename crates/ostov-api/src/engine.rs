use std::sync::Arc;

use crate::{
    EngineError, EngineResult, Fields, HelloInfo, JobId, JournalEntry, ListPage, ListQuery, Meta,
    NewUser, ObjectData, PostAction, ReportQuery, Saved, SessionInfo, UserInfo,
};

/// Контракт интерфейса с ядром. Методы синхронные: интерфейс зовёт их из фонового потока.
/// По умолчанию каждая команда отвечает «не реализовано», реализация переопределяет нужные.
pub trait Engine: Send + Sync {
    fn hello(&self) -> EngineResult<HelloInfo> {
        Err(EngineError::not_implemented("Hello"))
    }
    fn login(&self, _login: &str, _password: &str) -> EngineResult<SessionInfo> {
        Err(EngineError::not_implemented("Login"))
    }
    fn logout(&self) -> EngineResult<()> {
        Err(EngineError::not_implemented("Logout"))
    }
    fn create_first_admin(&self, _login: &str, _password: &str) -> EngineResult<SessionInfo> {
        Err(EngineError::not_implemented("CreateFirstAdmin"))
    }
    fn get_meta(&self) -> EngineResult<Meta> {
        Err(EngineError::not_implemented("GetMeta"))
    }

    fn list(&self, _query: &ListQuery) -> EngineResult<ListPage> {
        Err(EngineError::not_implemented("List"))
    }
    /// Окно, начиная со строки `position` (с нуля): страница N — `(N - 1) × size`,
    /// ползунок — доля × `total`. `token` — из ответа `list`.
    fn seek(&self, _token: &str, _position: u64, _size: u32) -> EngineResult<ListPage> {
        Err(EngineError::not_implemented("Seek"))
    }

    fn get(&self, _object: &str, _id: i64) -> EngineResult<ObjectData> {
        Err(EngineError::not_implemented("Get"))
    }
    /// Запись с проверкой `data.version`; для документа — сразу провести или снять
    /// с проведения в той же транзакции.
    fn save(&self, _object: &str, _data: &ObjectData, _post: PostAction) -> EngineResult<Saved> {
        Err(EngineError::not_implemented("Save"))
    }
    /// Пометка на удаление или её снятие (СТ-30: удаление — это пометка).
    fn mark_deleted(
        &self,
        _object: &str,
        _id: i64,
        _version: i64,
        _deleted: bool,
    ) -> EngineResult<Saved> {
        Err(EngineError::not_implemented("MarkDeleted"))
    }
    fn post(&self, _document: &str, _id: i64) -> EngineResult<Saved> {
        Err(EngineError::not_implemented("Post"))
    }
    fn unpost(&self, _document: &str, _id: i64) -> EngineResult<Saved> {
        Err(EngineError::not_implemented("Unpost"))
    }

    fn run_report(&self, _query: &ReportQuery) -> EngineResult<ListPage> {
        Err(EngineError::not_implemented("RunReport"))
    }
    fn start_import(&self, _format: &str, _source: &str) -> EngineResult<JobId> {
        Err(EngineError::not_implemented("Import"))
    }
    fn start_export(&self, _format: &str, _target: &str) -> EngineResult<JobId> {
        Err(EngineError::not_implemented("Export"))
    }

    /// Настройки пользователя по ключу (вид списка, колонки, избранное); хранятся в базе.
    fn get_settings(&self, _key: &str) -> EngineResult<Option<Fields>> {
        Err(EngineError::not_implemented("GetSettings"))
    }
    fn save_settings(&self, _key: &str, _settings: &Fields) -> EngineResult<()> {
        Err(EngineError::not_implemented("SaveSettings"))
    }

    fn admin_users(&self) -> EngineResult<Vec<UserInfo>> {
        Err(EngineError::not_implemented("AdminUsers"))
    }
    fn admin_add_user(&self, _user: &NewUser) -> EngineResult<()> {
        Err(EngineError::not_implemented("AdminAddUser"))
    }
    fn admin_set_user_active(&self, _login: &str, _active: bool) -> EngineResult<()> {
        Err(EngineError::not_implemented("AdminSetUserActive"))
    }
    fn admin_journal(
        &self,
        _after_key: Option<&str>,
        _size: u32,
    ) -> EngineResult<Vec<JournalEntry>> {
        Err(EngineError::not_implemented("AdminJournal"))
    }
    fn admin_backup(&self, _target: &str) -> EngineResult<JobId> {
        Err(EngineError::not_implemented("AdminBackup"))
    }

    /// Дата закрытия периода (СТ-24): документы по эту дату включительно не меняются.
    fn closing_date(&self) -> EngineResult<Option<String>> {
        Err(EngineError::not_implemented("GetClosingDate"))
    }
    /// `None` — открыть период. Потом — только с правом «установка даты закрытия» (СТ-30).
    fn set_closing_date(&self, _date: Option<&str>) -> EngineResult<()> {
        Err(EngineError::not_implemented("SetClosingDate"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdmin {
    pub login: String,
    pub password: String,
}

/// Открывает или создаёт базу и возвращает готовый `Engine`.
pub trait Connector: Send + Sync {
    fn open_local(
        &self,
        _path: &str,
        _file_password: Option<&str>,
    ) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("OpenLocal"))
    }
    fn create_local(
        &self,
        _path: &str,
        _file_password: Option<&str>,
        _admin: Option<&NewAdmin>,
    ) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("CreateLocal"))
    }
    fn connect_remote(&self, _address: &str) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("ConnectRemote"))
    }
    fn create_remote(
        &self,
        _address: &str,
        _database: &str,
        _admin: &NewAdmin,
    ) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("CreateRemote"))
    }
}

/// Заглушка: все команды отвечают «не реализовано».
#[derive(Debug, Default, Clone, Copy)]
pub struct StubEngine;

impl Engine for StubEngine {}

#[derive(Debug, Default, Clone, Copy)]
pub struct StubConnector;

impl Connector for StubConnector {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    #[test]
    fn stub_answers_not_implemented() {
        let engine: Arc<dyn Engine> = Arc::new(StubEngine);
        assert_eq!(engine.hello().unwrap_err().code, ErrorCode::NotImplemented);
        assert_eq!(
            engine.get("goods", 1).unwrap_err().code,
            ErrorCode::NotImplemented
        );
        let open = StubConnector.open_local("shop.db", None);
        assert_eq!(open.err().map(|e| e.code), Some(ErrorCode::NotImplemented));
    }
}
