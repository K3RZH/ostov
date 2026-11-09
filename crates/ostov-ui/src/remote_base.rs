//! Сервер (режим 2) через `ostov-client`: проверка адреса и сертификата, подключение, первый
//! администратор. Сертификат сервера (`server.crt`) копируется в папку настроек и дальше берётся
//! оттуда, поэтому исходный файл можно переместить. Клиент доверяет только этому сертификату.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ostov_api::{Connector, Engine, EngineError, EngineResult, ErrorCode, HelloInfo, NewAdmin};
use ostov_client::RemoteConnector;

/// Начало PEM-сертификата: по нему отличаем сертификат от случайного файла.
const PEM_MARK: &str = "-----BEGIN CERTIFICATE-----";

fn invalid(message: impl Into<String>) -> EngineError {
    EngineError::new(ErrorCode::Validation, message)
}

fn unavailable(message: impl Into<String>) -> EngineError {
    EngineError::new(ErrorCode::Unavailable, message)
}

/// Читает сертификат сервера из выбранного пользователем файла.
///
/// # Errors
/// Файл не читается или в нём нет сертификата.
pub fn read_certificate(path: &str) -> EngineResult<String> {
    let path = crate::local_base::expand_home(path);
    let pem = std::fs::read_to_string(&path)
        .map_err(|error| invalid(format!("Не удалось прочитать файл сертификата: {error}.")))?;
    if !pem.contains(PEM_MARK) {
        return Err(invalid(
            "Это не сертификат сервера: нужен файл server.crt, который сервер создаёт при первом запуске.",
        ));
    }
    Ok(pem)
}

/// Подключается к серверу с этим сертификатом и спрашивает `hello`: первый запуск или обычный
/// вход, какая конфигурация. Чужой сертификат или недоступный адрес дают ошибку ядра.
///
/// # Errors
/// Ошибка клиента как есть: неверный адрес, нет связи, сертификат не тот.
pub fn check(address: &str, pem: &str) -> EngineResult<(Arc<dyn Engine>, HelloInfo)> {
    let engine = RemoteConnector::new(pem).connect_remote(address.trim())?;
    let hello = engine.hello()?;
    Ok((engine, hello))
}

/// Первый запуск сервера: создаёт администратора, сессия уже открыта под ним. Пароль не короче
/// 12 символов проверяет сервер (СТ-32), здесь его не смягчить.
///
/// # Errors
/// Администратор уже есть, пароль не подошёл, нет связи.
pub fn create(address: &str, pem: &str, admin: &NewAdmin) -> EngineResult<Arc<dyn Engine>> {
    RemoteConnector::new(pem).create_remote(address.trim(), "", admin)
}

/// Имя файла сертификата для адреса. Символы кроме букв, цифр, точки и дефиса заменяются
/// кодом байта: разные адреса не совпадут, а имя безопасно для файловой системы.
fn certificate_name(address: &str) -> String {
    let mut name = String::new();
    for byte in address.trim().bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' {
            name.push(char::from(byte));
        } else {
            name.push_str(&format!("_{byte:02x}"));
        }
    }
    format!("{name}.crt")
}

fn certificates_dir() -> EngineResult<PathBuf> {
    crate::settings::config_file("servers").ok_or_else(|| {
        unavailable("Не найдена папка настроек: сертификат сервера некуда сохранить.")
    })
}

/// Сохраняет сертификат сервера в папке настроек.
///
/// # Errors
/// Папку не создать или файл не записать.
pub fn store_certificate(address: &str, pem: &str) -> EngineResult<()> {
    store_certificate_in(&certificates_dir()?, address, pem)
}

/// Сертификат сервера, сохранённый при добавлении.
///
/// # Errors
/// Сертификата нет (сервер надо добавить заново) или файл не читается.
pub fn stored_certificate(address: &str) -> EngineResult<String> {
    stored_certificate_in(&certificates_dir()?, address)
}

fn store_certificate_in(dir: &Path, address: &str, pem: &str) -> EngineResult<()> {
    std::fs::create_dir_all(dir)
        .map_err(|error| unavailable(format!("Не удалось создать папку сертификатов: {error}.")))?;
    std::fs::write(dir.join(certificate_name(address)), pem)
        .map_err(|error| unavailable(format!("Не удалось сохранить сертификат сервера: {error}.")))
}

fn stored_certificate_in(dir: &Path, address: &str) -> EngineResult<String> {
    std::fs::read_to_string(dir.join(certificate_name(address))).map_err(|_| {
        unavailable(
            "Сертификат этого сервера не найден: добавьте сервер заново и выберите server.crt.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ostov-ui-remote-{name}-{}", std::process::id()))
    }

    #[test]
    fn certificate_names_differ_and_are_safe() {
        let a = certificate_name("server.local:7700");
        let b = certificate_name("server.local:7701");
        let c = certificate_name("кафе");
        assert_ne!(a, b);
        assert_ne!(certificate_name("a/b"), certificate_name("a_b"));
        for name in [&a, &b, &c] {
            assert!(name.ends_with(".crt"));
            assert!(
                name.chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_')),
                "{name}"
            );
        }
        assert!(!certificate_name("../etc/passwd").contains('/'));
    }

    #[test]
    fn stored_certificate_round_trips() {
        let dir = temp_dir("store");
        let pem = format!("{PEM_MARK}\nabc\n-----END CERTIFICATE-----\n");
        assert!(store_certificate_in(&dir, "srv:7700", &pem).is_ok());
        assert_eq!(
            stored_certificate_in(&dir, "srv:7700").ok().as_deref(),
            Some(pem.as_str())
        );
        assert!(stored_certificate_in(&dir, "other:7700").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_certificates_are_accepted() {
        let dir = temp_dir("read");
        let _ = std::fs::create_dir_all(&dir);
        let good = dir.join("server.crt");
        let bad = dir.join("notes.txt");
        let _ = std::fs::write(&good, format!("{PEM_MARK}\nabc\n"));
        let _ = std::fs::write(&bad, "просто текст");
        assert!(read_certificate(&good.to_string_lossy()).is_ok());
        let error = read_certificate(&bad.to_string_lossy());
        assert!(
            matches!(error, Err(ref e) if e.code == ErrorCode::Validation),
            "{error:?}"
        );
        assert!(read_certificate(&dir.join("нет.crt").to_string_lossy()).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
