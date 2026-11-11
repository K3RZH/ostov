//! Вложенные транзакции: внешняя — `BEGIN`/`COMMIT`, внутренние — точки сохранения.
//! Так функция с транзакцией работает и сама по себе, и внутри чужой (например, запись
//! журнала в одной транзакции с изменением).

#[derive(Debug, Default)]
pub(crate) struct Depth(u32);

impl Depth {
    pub(crate) fn begin(&self) -> String {
        match self.0 {
            0 => "BEGIN".to_owned(),
            n => format!("SAVEPOINT ostov_tx_{n}"),
        }
    }

    pub(crate) fn commit(&self) -> String {
        match self.0 {
            0 | 1 => "COMMIT".to_owned(),
            n => format!("RELEASE SAVEPOINT ostov_tx_{}", n - 1),
        }
    }

    pub(crate) fn rollback(&self) -> String {
        match self.0 {
            0 | 1 => "ROLLBACK".to_owned(),
            n => format!(
                "ROLLBACK TO SAVEPOINT ostov_tx_{0}; RELEASE SAVEPOINT ostov_tx_{0}",
                n - 1
            ),
        }
    }

    pub(crate) fn entered(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    pub(crate) fn left(&mut self) {
        self.0 = self.0.saturating_sub(1);
    }

    /// Вне всякой транзакции.
    pub(crate) fn is_outside(&self) -> bool {
        self.0 == 0
    }
}
