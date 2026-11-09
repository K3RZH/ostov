//! TLS обязателен (СТ-33): без сертификата сервер не запускается. При первом запуске сервер
//! сам создаёт самоподписанный сертификат на свои имена; клиенты доверяют именно ему —
//! файл `server.crt` администратор раздаёт на рабочие места (ADR-021).

use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub const CERT_FILE: &str = "server.crt";
pub const KEY_FILE: &str = "server.key";

#[derive(Clone)]
pub struct Tls {
    pub cert_pem: String,
    pub key_pem: String,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tls(***)")
    }
}

impl Tls {
    /// Сертификат и ключ из `dir`; если их нет — создаёт на имена `hosts`
    /// (например `localhost`, имя компьютера, его адрес в сети) и сохраняет туда же.
    pub fn load_or_create(dir: &Path, hosts: &[String]) -> io::Result<Self> {
        let (cert, key) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if cert.exists() && key.exists() {
            return Ok(Self {
                cert_pem: fs::read_to_string(cert)?,
                key_pem: fs::read_to_string(key)?,
            });
        }
        let made = rcgen::generate_simple_self_signed(hosts.to_vec())
            .map_err(|e| io::Error::other(format!("не удалось создать сертификат: {e}")))?;
        let tls = Self {
            cert_pem: made.cert.pem(),
            key_pem: made.signing_key.serialize_pem(),
        };
        fs::create_dir_all(dir)?;
        write_key(&key, &tls.key_pem)?;
        fs::write(&cert, &tls.cert_pem)?;
        Ok(tls)
    }
}

/// Закрытый ключ читает только владелец (на Linux права 600).
fn write_key(path: &Path, pem: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(pem.as_bytes())
}
