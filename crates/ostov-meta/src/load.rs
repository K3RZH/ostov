//! Загрузка папки конфигурации: манифест, затем справочники, документы и регистры.

use std::borrow::Cow;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_saphyr::budget::BudgetBreach;
use serde_saphyr::{
    Localizer, Location, MergeKeyPolicy, MessageFormatter, Options, RenderOptions, SnippetMode,
    Spanned, UserMessageFormatter,
};

use crate::{ConfigFiles, Error, Located};

/// Версия формата файлов, которую читает эта сборка. Другая версия отвергается (СТ-07).
pub const FORMAT_VERSION: u32 = 1;

pub(crate) const MANIFEST_FILE: &str = "ostov.yaml";
pub(crate) const CATALOGS_DIR: &str = "catalogs";
pub(crate) const DOCUMENTS_DIR: &str = "documents";
pub(crate) const REGISTERS_DIR: &str = "registers";
pub(crate) const SUBSYSTEMS_DIR: &str = "subsystems";
const EXTENSION: &str = "yaml";

/// Файл описывает один объект: даже большой справочник занимает килобайты, 1 МБ — с огромным запасом.
const MAX_FILE_BYTES: usize = 1024 * 1024;
/// Самая глубокая вложенность формата — поле табличной части документа, 5 уровней; запас втрое.
const MAX_DEPTH: usize = 16;

/// Читает и проверяет конфигурацию из папки `dir` и отдаёт модель для ядра.
///
/// # Errors
/// Сначала ошибки разбора: пока файл не разобран, ссылки на его объекты проверять нельзя.
/// Если разбор прошёл — все ошибки [`crate::check`]. У каждой ошибки файл, строка и причина.
pub fn load(dir: &Path) -> Result<ostov_model::Config, Vec<Error>> {
    checked(parse(dir)?).map(crate::convert::into_model)
}

/// Читает папку `dir` как есть, с номерами строк, без проверки ссылок и имён.
///
/// # Errors
/// Все ошибки разбора сразу. При ошибке в манифесте остальные файлы не читаются:
/// без верной версии формата их нельзя разобрать.
pub fn parse(dir: &Path) -> Result<ConfigFiles, Vec<Error>> {
    let mut errors = Vec::new();
    let sources = Sources {
        manifest: Source::file(dir.join(MANIFEST_FILE)),
        catalogs: folder_sources(&dir.join(CATALOGS_DIR), &mut errors),
        documents: folder_sources(&dir.join(DOCUMENTS_DIR), &mut errors),
        registers: folder_sources(&dir.join(REGISTERS_DIR), &mut errors),
        subsystems: folder_sources(&dir.join(SUBSYSTEMS_DIR), &mut errors),
    };
    parse_sources(&sources, errors)
}

pub(crate) fn checked(files: ConfigFiles) -> Result<ConfigFiles, Vec<Error>> {
    let errors = crate::check(&files);
    if errors.is_empty() {
        Ok(files)
    } else {
        Err(errors)
    }
}

/// Файл конфигурации: на диске или встроенный в программу (`demo()`).
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    /// Текст встроенного файла; `None` — читать с диска по `path`.
    pub(crate) text: Option<&'static str>,
}

impl Source {
    fn file(path: PathBuf) -> Self {
        Self { path, text: None }
    }
}

pub(crate) struct Sources {
    pub(crate) manifest: Source,
    pub(crate) catalogs: Vec<Source>,
    pub(crate) documents: Vec<Source>,
    pub(crate) registers: Vec<Source>,
    pub(crate) subsystems: Vec<Source>,
}

/// Разбирает манифест, затем все файлы объектов; `errors` — уже найденные ошибки чтения папок.
pub(crate) fn parse_sources(
    sources: &Sources,
    mut errors: Vec<Error>,
) -> Result<ConfigFiles, Vec<Error>> {
    let manifest = parse_manifest(&sources.manifest).map_err(|error| vec![error])?;
    let catalogs = parse_all(&sources.catalogs, &mut errors);
    let documents = parse_all(&sources.documents, &mut errors);
    let registers = parse_all(&sources.registers, &mut errors);
    let subsystems = parse_all(&sources.subsystems, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ConfigFiles {
        manifest,
        catalogs,
        documents,
        registers,
        subsystems,
    })
}

/// Только версия формата: её читаем раньше строгого разбора, чтобы файл нового формата
/// дал понятную ошибку о версии, а не «неизвестный ключ».
#[derive(Deserialize)]
struct FormatVersion {
    format: Spanned<u32>,
}

fn parse_manifest(source: &Source) -> Result<Located<crate::Manifest>, Error> {
    let path = &source.path;
    if source.text.is_none() && !path.is_file() {
        return Err(Error::new(path, None, "нет манифеста конфигурации"));
    }
    let version: FormatVersion = parse_source(source)?;
    if version.format.value != FORMAT_VERSION {
        return Err(Error::new(
            path,
            Some(version.format.referenced.line()),
            format!(
                "формат конфигурации {} не поддерживается: эта версия ostov читает формат {FORMAT_VERSION}",
                version.format.value
            ),
        ));
    }
    Ok(Located {
        file: path.clone(),
        value: parse_source(source)?,
    })
}

fn parse_all<T: DeserializeOwned>(sources: &[Source], errors: &mut Vec<Error>) -> Vec<Located<T>> {
    let mut objects = Vec::new();
    for source in sources {
        match parse_source(source) {
            Ok(value) => objects.push(Located {
                file: source.path.clone(),
                value,
            }),
            Err(error) => errors.push(error),
        }
    }
    objects
}

/// Все `*.yaml` папки по алфавиту. Папки нет — объектов этого вида нет.
fn folder_sources(dir: &Path, errors: &mut Vec<Error>) -> Vec<Source> {
    if !dir.exists() {
        return Vec::new();
    }
    match yaml_files(dir) {
        Ok(files) => files.into_iter().map(Source::file).collect(),
        Err(error) => {
            errors.push(Error::new(
                dir,
                None,
                format!("не удалось прочитать папку: {error}"),
            ));
            Vec::new()
        }
    }
}

fn yaml_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == EXTENSION) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_source<T: DeserializeOwned>(source: &Source) -> Result<T, Error> {
    let path = &source.path;
    let parsed = match source.text {
        // Встроенный текст — наш собственный файл, лимит размера для него не нужен.
        Some(text) => serde_saphyr::from_str_with_options(text, strict_options()),
        None => {
            let file = File::open(path).map_err(|error| {
                Error::new(path, None, format!("не удалось открыть файл: {error}"))
            })?;
            serde_saphyr::from_reader_with_options(file, strict_options())
        }
    };
    parsed.map_err(|error| yaml_error(path, &error))
}

fn strict_options() -> Options {
    serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            max_reader_input_bytes: Some(MAX_FILE_BYTES),
            max_depth: MAX_DEPTH,
            max_anchors: 0,
            max_aliases: 0,
        },
        merge_keys: MergeKeyPolicy::Error,
    }
}

fn yaml_error(path: &Path, error: &serde_saphyr::Error) -> Error {
    let line = error
        .location()
        .filter(|location| *location != Location::UNKNOWN)
        .map(|location| location.line());
    Error::new(path, line, reason(error.without_snippet()))
}

/// Нарушения строгих правил формата объясняем по-русски, остальное — текстом serde-saphyr.
fn reason(error: &serde_saphyr::Error) -> String {
    match error {
        serde_saphyr::Error::Budget {
            breach: BudgetBreach::Anchors { .. } | BudgetBreach::Aliases { .. },
            ..
        } => "якоря и алиасы (&имя, *имя) запрещены: напиши значение явно".into(),
        serde_saphyr::Error::Budget {
            breach: BudgetBreach::Depth { .. },
            ..
        } => format!("вложенность глубже {MAX_DEPTH} уровней"),
        serde_saphyr::Error::IOError { cause } if cause.kind() == ErrorKind::FileTooLarge => {
            format!("файл больше {} МБ", MAX_FILE_BYTES / (1024 * 1024))
        }
        serde_saphyr::Error::SerdeUnknownField {
            field, expected, ..
        } => format!(
            "неизвестный ключ «{field}»: допустимы {}",
            expected.join(", ")
        ),
        serde_saphyr::Error::SerdeMissingField { field, .. } => {
            format!("нет обязательного ключа «{field}»")
        }
        serde_saphyr::Error::SerdeUnknownVariant {
            variant, expected, ..
        } => format!(
            "неизвестное значение «{variant}»: допустимо {}",
            expected.join(", ")
        ),
        serde_saphyr::Error::DuplicateMappingKey { key: Some(key), .. } => {
            format!("ключ «{key}» повторяется")
        }
        serde_saphyr::Error::UnknownAnchor { .. } => {
            "алиас ссылается на неизвестный якорь; якоря и алиасы запрещены".into()
        }
        _ => {
            let mut options = RenderOptions::default();
            options.formatter = &BareMessage;
            options.snippets = SnippetMode::Off;
            error.render_with_options(options)
        }
    }
}

/// Текст ошибки без «at line X, column Y»: место мы показываем сами, как «файл:строка».
struct BareMessage;

impl MessageFormatter for BareMessage {
    fn localizer(&self) -> &dyn Localizer {
        self
    }

    fn format_message<'a>(&self, error: &'a serde_saphyr::Error) -> Cow<'a, str> {
        UserMessageFormatter.format_message(error)
    }
}

impl Localizer for BareMessage {
    fn attach_location<'a>(&self, base: Cow<'a, str>, _location: Location) -> Cow<'a, str> {
        base
    }
}
