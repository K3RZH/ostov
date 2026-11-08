//! Логика конфигурации на Python (ADR-003, ADR-018). Пока здесь только проба упаковки:
//! `examples/probe.rs`. PyO3 подключается фичей `python`, по умолчанию выключенной.

/// Собран ли крейт с Python.
pub const PYTHON_ENABLED: bool = cfg!(feature = "python");
