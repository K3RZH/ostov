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

use crate::{Config, Error, Located};

/// Версия формата файлов, которую читает эта сборка. Другая версия отвергается (СТ-07).
pub const FORMAT_VERSION: u32 = 1;

const MANIFEST_FILE: &str = "ostov.yaml";
const CATALOGS_DIR: &str = "catalogs";
const DOCUMENTS_DIR: &str = "documents";
const REGISTERS_DIR: &str = "registers";
const EXTENSION: &str = "yaml";

/// Файл описывает один объект: даже большой справочник занимает килобайты, 1 МБ — с огромным запасом.
const MAX_FILE_BYTES: usize = 1024 * 1024;
/// Самая глубокая вложенность формата — поле табличной части документа, 5 уровней; запас втрое.
const MAX_DEPTH: usize = 16;

/// Читает конфигурацию из папки `dir`.
///
/// # Errors
/// Все найденные ошибки разбора сразу, у каждой файл, строка и причина. При ошибке в манифесте
/// остальные файлы не читаются: без верной версии формата их нельзя разобрать.
pub fn load(dir: &Path) -> Result<Config, Vec<Error>> {
    let manifest = load_manifest(&dir.join(MANIFEST_FILE)).map_err(|error| vec![error])?;
    let mut errors = Vec::new();
    let catalogs = load_folder(&dir.join(CATALOGS_DIR), &mut errors);
    let documents = load_folder(&dir.join(DOCUMENTS_DIR), &mut errors);
    let registers = load_folder(&dir.join(REGISTERS_DIR), &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Config {
        manifest,
        catalogs,
        documents,
        registers,
    })
}

/// Только версия формата: её читаем раньше строгого разбора, чтобы файл нового формата
/// дал понятную ошибку о версии, а не «неизвестный ключ».
#[derive(Deserialize)]
struct FormatVersion {
    format: Spanned<u32>,
}

fn load_manifest(path: &Path) -> Result<Located<crate::Manifest>, Error> {
    if !path.is_file() {
        return Err(Error::new(path, None, "нет манифеста конфигурации"));
    }
    let version: FormatVersion = parse_file(path)?;
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
        file: path.to_owned(),
        value: parse_file(path)?,
    })
}

/// Читает все `*.yaml` папки по алфавиту. Папки нет — объектов этого вида нет.
fn load_folder<T: DeserializeOwned>(dir: &Path, errors: &mut Vec<Error>) -> Vec<Located<T>> {
    if !dir.exists() {
        return Vec::new();
    }
    let files = match yaml_files(dir) {
        Ok(files) => files,
        Err(error) => {
            errors.push(Error::new(
                dir,
                None,
                format!("не удалось прочитать папку: {error}"),
            ));
            return Vec::new();
        }
    };
    let mut objects = Vec::new();
    for file in files {
        match parse_file(&file) {
            Ok(value) => objects.push(Located { file, value }),
            Err(error) => errors.push(error),
        }
    }
    objects
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

fn parse_file<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let file = File::open(path)
        .map_err(|error| Error::new(path, None, format!("не удалось открыть файл: {error}")))?;
    serde_saphyr::from_reader_with_options(file, strict_options())
        .map_err(|error| yaml_error(path, &error))
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
