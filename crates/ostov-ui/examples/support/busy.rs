//! Подставное ядро для стендов: всё передаёт настоящему, но запись и действия над строкой
//! отвечают «данные заняты» (`ErrorCode::Unavailable`) столько раз подряд, сколько попросили.
//! Так ядро отвечает, когда сам уже трижды повторил и не дождался чужой транзакции.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ostov_api::{
    Engine, EngineError, EngineResult, ErrorCode, ListPage, ListQuery, Meta, ObjectData,
    PostAction, Saved,
};

pub struct BusyEngine {
    inner: Arc<dyn Engine>,
    failures: AtomicU32,
}

impl BusyEngine {
    pub fn new(inner: Arc<dyn Engine>) -> Self {
        Self {
            inner,
            failures: AtomicU32::new(0),
        }
    }

    /// Следующие `count` записей или действий над строкой не пройдут: данные заняты.
    pub fn fail_next(&self, count: u32) {
        self.failures.store(count, Ordering::SeqCst);
    }

    fn busy(&self) -> EngineResult<()> {
        match self
            .failures
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            }) {
            Ok(_) => Err(EngineError {
                code: ErrorCode::Unavailable,
                message: "Эти данные сейчас меняет другой пользователь. Повторите действие."
                    .to_owned(),
                details: "busy: стенд".to_owned(),
            }),
            Err(_) => Ok(()),
        }
    }
}

impl Engine for BusyEngine {
    fn get_meta(&self) -> EngineResult<Meta> {
        self.inner.get_meta()
    }
    fn list(&self, query: &ListQuery) -> EngineResult<ListPage> {
        self.inner.list(query)
    }
    fn seek(&self, token: &str, position: u64, size: u32) -> EngineResult<ListPage> {
        self.inner.seek(token, position, size)
    }
    fn get(&self, object: &str, id: i64) -> EngineResult<ObjectData> {
        self.inner.get(object, id)
    }
    fn save(&self, object: &str, data: &ObjectData, post: PostAction) -> EngineResult<Saved> {
        self.busy()?;
        self.inner.save(object, data, post)
    }
    fn mark_deleted(
        &self,
        object: &str,
        id: i64,
        version: i64,
        deleted: bool,
    ) -> EngineResult<Saved> {
        self.busy()?;
        self.inner.mark_deleted(object, id, version, deleted)
    }
    fn post(&self, document: &str, id: i64) -> EngineResult<Saved> {
        self.busy()?;
        self.inner.post(document, id)
    }
    fn unpost(&self, document: &str, id: i64) -> EngineResult<Saved> {
        self.busy()?;
        self.inner.unpost(document, id)
    }
}
