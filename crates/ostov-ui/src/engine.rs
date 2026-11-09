//! Связь интерфейса с ядром. Контракт (трейт `Engine`, `Connector`, типы и ошибки) живёт в
//! `ostov-api` (согласован с Никитой); здесь он реэкспортируется и лежит помощник для вызова
//! ядра из фонового потока. Какое место интерфейса какой команде соответствует, описано в
//! `Вызовы-ядра-из-ostov-ui.md` в корне репозитория.
//!
//! Режим 1 (файл): `ostov_core::LocalConnector` даёт `Arc<dyn Engine>`; режим 2 позже.

pub use ostov_api::*;

// ---------------------------------------------------------------- фоновый вызов

/// Выполняет вызов ядра в фоновом потоке и отдаёт результат в поток интерфейса, где его можно
/// безопасно положить в свойства окна. Так отрисовка не блокируется долгим запросом (§3.1).
///
/// ```ignore
/// let engine = engine.clone();
/// run_in_background(app.as_weak(), move || engine.list(&query), |app, result| {
///     match result {
///         Ok(page) => { /* показать строки */ }
///         Err(error) => { /* показать error.message */ }
///     }
/// });
/// ```
pub fn run_in_background<T, C, F, D>(window: slint::Weak<C>, work: F, done: D)
where
    T: Send + 'static,
    C: slint::ComponentHandle + 'static,
    F: FnOnce() -> EngineResult<T> + Send + 'static,
    D: FnOnce(C, EngineResult<T>) + Send + 'static,
{
    std::thread::spawn(move || {
        let result = work();
        // Если окно уже закрыто, результат некому показывать.
        let _ = window.upgrade_in_event_loop(move |window| done(window, result));
    });
}

/// Вызов ядра в фоновом потоке с результатом в потоке интерфейса, когда `apply` нужен доступ к
/// состоянию, которое нельзя отправлять между потоками (`Rc`). Результат забирает таймер
/// интерфейса: опрос раз в 16 мс, пока ответа нет. Если поток упал, не ответив, `apply` получает
/// ошибку: иначе кнопка, ждущая ответа, осталась бы выключенной навсегда.
pub fn call_in_background<T, F, D>(work: F, apply: D)
where
    T: Send + 'static,
    F: FnOnce() -> EngineResult<T> + Send + 'static,
    D: FnOnce(EngineResult<T>) + 'static,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Получателя уже может не быть (окно закрыто): тогда отправлять некому.
        let _ = sender.send(work());
    });
    poll_result(receiver, apply);
}

fn poll_result<T, D>(receiver: std::sync::mpsc::Receiver<EngineResult<T>>, apply: D)
where
    T: Send + 'static,
    D: FnOnce(EngineResult<T>) + 'static,
{
    use std::sync::mpsc::TryRecvError;
    slint::Timer::single_shot(std::time::Duration::from_millis(16), move || match receiver
        .try_recv()
    {
        Ok(result) => apply(result),
        Err(TryRecvError::Empty) => poll_result(receiver, apply),
        Err(TryRecvError::Disconnected) => apply(Err(EngineError::new(
            ErrorCode::Internal,
            "Внутренняя ошибка: операция прервалась. Повторите действие.",
        ))),
    });
}
