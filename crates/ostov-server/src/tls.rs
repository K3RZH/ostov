//! TLS обязателен (СТ-33): без сертификата сервер не запускается. При первом запуске сервер
//! создаёт свой корневой центр `ca.crt` и сертификат сервера от него на свои имена (ADR-021).
//! Клиентам раздают только `ca.crt`: при новом имени или адресе сервера сертификат сервера
//! выпускается заново тем же центром, и на рабочих местах ничего менять не нужно.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};

/// Корневой сертификат: его кладут клиентам.
pub const CA_FILE: &str = "ca.crt";
const CA_KEY_FILE: &str = "ca.key";
pub const CERT_FILE: &str = "server.crt";
pub const KEY_FILE: &str = "server.key";
/// Имена, на которые выписан `server.crt`, по одному в строке.
const HOSTS_FILE: &str = "server.hosts";

#[derive(Clone)]
pub struct Tls {
    pub cert_pem: String,
    pub key_pem: String,
    /// Чему доверяет клиент: `ca.crt`, а у установки до корневого центра — сам `server.crt`.
    pub ca_pem: String,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tls(***)")
    }
}

impl Tls {
    /// Сертификаты из `dir`; чего нет — создаёт. `hosts` — имена и адреса сервера
    /// (`localhost`, имя компьютера, адрес в сети): если среди них есть новые, сертификат
    /// сервера выпускается заново на старые и новые имена вместе.
    pub fn load_or_create(dir: &Path, hosts: &[String]) -> io::Result<Self> {
        let path = |name: &str| dir.join(name);
        let read = |name: &str| fs::read_to_string(path(name));
        // Установка до корневого центра: клиентам уже роздан server.crt, его и держим.
        if !path(CA_KEY_FILE).exists() && path(CERT_FILE).exists() && path(KEY_FILE).exists() {
            let cert_pem = read(CERT_FILE)?;
            return Ok(Self {
                ca_pem: cert_pem.clone(),
                cert_pem,
                key_pem: read(KEY_FILE)?,
            });
        }
        fs::create_dir_all(dir)?;
        let ca_key = if path(CA_KEY_FILE).exists() && path(CA_FILE).exists() {
            KeyPair::from_pem(&read(CA_KEY_FILE)?).map_err(failed)?
        } else {
            let key = KeyPair::generate().map_err(failed)?;
            let ca = ca_params().self_signed(&key).map_err(failed)?;
            write_key(&path(CA_KEY_FILE), &key.serialize_pem())?;
            fs::write(path(CA_FILE), ca.pem())?;
            // Новый центр — старый сертификат сервера им не подписан.
            let _ = fs::remove_file(path(HOSTS_FILE));
            key
        };
        let ca_pem = read(CA_FILE)?;

        let had: BTreeSet<String> = read(HOSTS_FILE)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .filter(|h| !h.is_empty())
            .collect();
        let ready = path(CERT_FILE).exists() && path(KEY_FILE).exists();
        if ready && !had.is_empty() && hosts.iter().all(|h| had.contains(h)) {
            return Ok(Self {
                cert_pem: read(CERT_FILE)?,
                key_pem: read(KEY_FILE)?,
                ca_pem,
            });
        }
        let names: BTreeSet<String> = had.into_iter().chain(hosts.iter().cloned()).collect();
        let names: Vec<String> = names.into_iter().collect();
        let mut params = CertificateParams::new(names.clone()).map_err(failed)?;
        params
            .distinguished_name
            .push(DnType::CommonName, "Ostov server");
        params.use_authority_key_identifier_extension = true;
        let key = KeyPair::generate().map_err(failed)?;
        let cert = params
            .signed_by(&key, &Issuer::new(ca_params(), ca_key))
            .map_err(failed)?;
        // В server.crt — цепочка с центром: клиент, которому дали server.crt, а не ca.crt,
        // тоже доверится центру.
        let tls = Self {
            cert_pem: format!("{}{ca_pem}", cert.pem()),
            key_pem: key.serialize_pem(),
            ca_pem,
        };
        write_key(&path(KEY_FILE), &tls.key_pem)?;
        fs::write(path(CERT_FILE), &tls.cert_pem)?;
        fs::write(path(HOSTS_FILE), names.join("\n"))?;
        Ok(tls)
    }
}

/// Параметры центра всегда одни: по ним и ключу из `ca.key` центр подписывает заново.
fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params
        .distinguished_name
        .push(DnType::CommonName, "Ostov root");
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params
}

fn failed(e: rcgen::Error) -> io::Error {
    io::Error::other(format!("не удалось создать сертификат: {e}"))
}

/// Закрытый ключ читает только владелец (на Linux права 600).
fn write_key(path: &Path, pem: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(pem.as_bytes())
}
