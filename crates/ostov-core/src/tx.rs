use ostov_store::Driver;

use crate::CoreError;

/// Выполняет `f` в одной транзакции: любая ошибка откатывает всё.
pub fn in_transaction<T>(
    db: &mut dyn Driver,
    f: impl FnOnce(&mut dyn Driver) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    db.begin()?;
    match f(db) {
        Ok(value) => {
            db.commit()?;
            Ok(value)
        }
        Err(e) => {
            // Возвращаем исходную ошибку: она важнее, чем возможная ошибка отката.
            let _ = db.rollback();
            Err(e)
        }
    }
}
