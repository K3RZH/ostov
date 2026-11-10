//! Встроенный интерпретатор: загрузка `.py` документов и правило проведения для ядра.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use ostov_core::local::Rules;
use ostov_core::{
    CoreError, Document, FieldKind, FieldValue, Fields, Movement, MovementKind, Moves, Parts,
    PostingRule, QueryRows, Reader,
};
use ostov_model::{Config, DocumentDef, FieldDef};
use ostov_sql::{Param, SqlError};
use ostov_store::Decimal;
use pyo3::IntoPyObjectExt;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyCFunction, PyDict, PyList, PyTuple};

use crate::{MODULE_PREFIX, PyLoadError, script_path};

/// Модуль `ostov` для конфигураций; кладётся в `sys.modules` один раз на процесс.
const RUNTIME_SOURCE: &str = include_str!("runtime.py");
static RUNTIME: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

type Kinds = Vec<(String, FieldKind)>;

/// Запрос из `ctx.query` потоку ядра и канал для ответа.
type Ask = (
    String,
    BTreeMap<String, Param>,
    Sender<Result<QueryRows, CoreError>>,
);

/// Связь `ctx.query` с ядром; после `on_post` пустая, поздний вызов — ошибка, а не зависание.
type Link = Arc<Mutex<Option<Sender<Ask>>>>;

/// Правила документов из `config/documents/*.py`.
pub struct PyRules {
    scripts: BTreeMap<String, Arc<Script>>,
}

impl PyRules {
    /// Загружает `config/documents/<документ>.py` для всех документов модели.
    ///
    /// # Errors
    /// Все ошибки сразу: нет Python рядом с программой, нет `.py` или `on_post` у документа
    /// с регистрами, синтаксическая ошибка или исключение при загрузке модуля.
    pub fn load(config_dir: &Path, model: &Config) -> Result<Self, Vec<PyLoadError>> {
        start_python().map_err(|error| vec![error])?;
        Python::attach(|py| {
            runtime(py).map_err(|e| {
                vec![PyLoadError::new(
                    "ostov.py",
                    None,
                    format!("модуль ostov: {e}"),
                )]
            })?;
            let mut scripts = BTreeMap::new();
            let mut errors = Vec::new();
            for document in &model.documents {
                match load_script(py, config_dir, model, document) {
                    Ok(Some(script)) => {
                        scripts.insert(document.name.clone(), Arc::new(script));
                    }
                    Ok(None) => {}
                    Err(error) => errors.push(error),
                }
            }
            if errors.is_empty() {
                Ok(Self { scripts })
            } else {
                Err(errors)
            }
        })
    }
}

impl std::fmt::Debug for PyRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRules")
            .field("documents", &self.scripts.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Rules for PyRules {
    fn rule(&self, document: &DocumentDef) -> Option<Box<dyn PostingRule>> {
        let script = self.scripts.get(&document.name)?;
        Some(Box::new(PyRule(Arc::clone(script))))
    }
}

/// Где Python ищет стандартную библиотеку: `PYTHONHOME` или папка программы.
fn python_home() -> Result<PathBuf, PyLoadError> {
    if let Some(home) = std::env::var_os("PYTHONHOME").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    let exe = std::env::current_exe()
        .map_err(|e| PyLoadError::new("", None, format!("не найдена программа: {e}")))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| PyLoadError::new(exe.clone(), None, "не найдена папка программы"))
}

/// Без своей стандартной библиотеки Python не ошибку вернёт, а завершит процесс.
/// Поэтому сначала проверяем `Lib/os.py`, и только потом запускаем интерпретатор.
fn start_python() -> Result<(), PyLoadError> {
    let home = python_home()?;
    let landmark = home.join("Lib").join("os.py");
    if !landmark.is_file() {
        return Err(PyLoadError::new(
            &home,
            None,
            format!(
                "рядом с программой нет Python: не найден {}",
                landmark.display()
            ),
        ));
    }
    Python::initialize();
    Ok(())
}

/// Модуль создаётся ровно один раз: параллельные загрузки ждут первую, иначе класс
/// `UserError` пересоздался бы и `isinstance` перестал бы его узнавать.
fn runtime(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let module = RUNTIME.get_or_try_init(py, || -> PyResult<Py<PyModule>> {
        let code = CString::new(RUNTIME_SOURCE)?;
        let module = PyModule::from_code(py, &code, c"ostov.py", c"ostov")?;
        py.import("sys")?
            .getattr("modules")?
            .set_item("ostov", &module)?;
        Ok(module.unbind())
    })?;
    Ok(module.bind(py).clone())
}

/// Логика одного документа и всё, что нужно для перевода данных в Python и обратно.
struct Script {
    name: String,
    title: String,
    header: Kinds,
    parts: Vec<(String, Kinds)>,
    /// Регистры модели: имя → (измерения, ресурсы).
    registers: Vec<(String, Kinds, Kinds)>,
    /// Регистры документа из YAML: двигать можно только их.
    allowed: Vec<String>,
    before_write: Option<Py<PyAny>>,
    on_post: Option<Py<PyAny>>,
}

fn load_script(
    py: Python<'_>,
    config_dir: &Path,
    model: &Config,
    document: &DocumentDef,
) -> Result<Option<Script>, PyLoadError> {
    let path = script_path(config_dir, &document.name);
    let file_name = format!("{}.py", document.name);
    if !path.is_file() {
        if document.registers.is_empty() {
            return Ok(None);
        }
        let reason = format!("нет {file_name}: у документа есть регистры, нужна функция on_post");
        return Err(PyLoadError::new(&path, None, reason));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| PyLoadError::new(&path, None, format!("не удалось прочитать файл: {e}")))?;
    let module = compile(py, &path, &text, &document.name)?;
    let function = |name: &str| -> Result<Option<Py<PyAny>>, PyLoadError> {
        let Ok(value) = module.getattr(name) else {
            return Ok(None);
        };
        if value.is_callable() {
            Ok(Some(value.unbind()))
        } else {
            Err(PyLoadError::new(
                &path,
                None,
                format!("{name} в {file_name} — не функция"),
            ))
        }
    };
    let on_post = function("on_post")?;
    let before_write = function("before_write")?;
    if on_post.is_none() && !document.registers.is_empty() {
        return Err(PyLoadError::new(
            &path,
            None,
            format!("в {file_name} нет on_post"),
        ));
    }
    let kinds = |fields: &[FieldDef]| -> Kinds {
        fields.iter().map(|f| (f.name.clone(), f.kind)).collect()
    };
    Ok(Some(Script {
        name: document.name.clone(),
        title: document.title.clone(),
        header: kinds(&document.fields),
        parts: document
            .table_parts
            .iter()
            .map(|p| (p.name.clone(), kinds(&p.fields)))
            .collect(),
        registers: model
            .registers
            .iter()
            .map(|r| (r.name.clone(), kinds(&r.dimensions), kinds(&r.resources)))
            .collect(),
        allowed: document.registers.clone(),
        before_write,
        on_post,
    }))
}

/// Каждая загрузка — свой объект модуля, код выполняется в его словарь. `PyModule::from_code`
/// так не умеет: он пишет в модуль из `sys.modules` по имени, и две загрузки одного документа
/// (повторная или из другого потока) подменили бы друг другу функции.
fn compile<'py>(
    py: Python<'py>,
    path: &Path,
    text: &str,
    document: &str,
) -> Result<Bound<'py, PyModule>, PyLoadError> {
    let file_name = path.to_string_lossy().into_owned();
    let run = || -> PyResult<Bound<'py, PyModule>> {
        let module = PyModule::new(py, &format!("{MODULE_PREFIX}{document}"))?;
        module.setattr("__file__", file_name.as_str())?;
        // В sys.modules — для библиотек, которые ищут там свой модуль (например, dataclasses).
        py.import("sys")?
            .getattr("modules")?
            .set_item(module.name()?, &module)?;
        let builtins = py.import("builtins")?;
        let code = builtins
            .getattr("compile")?
            .call1((text, file_name.as_str(), "exec"))?;
        builtins.getattr("exec")?.call1((code, module.dict()))?;
        Ok(module)
    };
    run().map_err(|error| {
        // Неудачный модуль не оставляем в sys.modules.
        let name = format!("{MODULE_PREFIX}{document}");
        let _ = py
            .import("sys")
            .and_then(|sys| sys.getattr("modules"))
            .and_then(|modules| modules.del_item(name));
        let described = runtime(py).and_then(|rt| {
            rt.getattr("_load_error")?
                .call1((error.value(py), file_name.as_str()))?
                .extract::<(Option<u64>, String)>()
        });
        match described {
            Ok((line, reason)) => PyLoadError::new(path, line, reason),
            Err(_) => PyLoadError::new(path, None, error.to_string()),
        }
    })
}

struct PyRule(Arc<Script>);

impl PyRule {
    fn post(&self, doc: &Document, asks: Option<Sender<Ask>>) -> Result<Moves, CoreError> {
        let script = &self.0;
        let mut moves: Moves = script
            .allowed
            .iter()
            .map(|r| (r.clone(), Vec::new()))
            .collect();
        let Some(on_post) = &script.on_post else {
            return Ok(moves);
        };
        let link: Link = Arc::new(Mutex::new(asks));
        let result = Python::attach(|py| {
            let query = query_function(py, Arc::clone(&link)).map_err(|e| script.error(py, &e))?;
            let rows = script
                .call_on_post(py, on_post, doc, query)
                .map_err(|e| script.error(py, &e))?;
            for (register, kind, values) in rows {
                let movement = script.movement(&register, &kind, &values)?;
                moves.entry(register).or_default().push(movement);
            }
            Ok(moves)
        });
        link.lock().unwrap_or_else(PoisonError::into_inner).take();
        result
    }
}

impl PostingRule for PyRule {
    fn movements(&self, doc: &Document) -> Result<Moves, CoreError> {
        self.post(doc, None)
    }

    /// `on_post` идёт в своём потоке, а этот держит транзакцию и выполняет его `ctx.query`:
    /// PyO3 берёт только `'static`-функции, а соединение с базой занято проведением.
    fn movements_reading(&self, doc: &Document, read: &mut dyn Reader) -> Result<Moves, CoreError> {
        let (asks, inbox) = mpsc::channel::<Ask>();
        thread::scope(|s| {
            let worker = s.spawn(move || self.post(doc, Some(asks)));
            for (sql, params, answer) in inbox {
                let _ = answer.send(read.query(&sql, &params));
            }
            worker.join().unwrap_or_else(|_| {
                Err(CoreError::Script {
                    message: format!(
                        "Ошибка в логике документа «{}»: поток Python упал",
                        self.0.title
                    ),
                    details: String::new(),
                })
            })
        })
    }

    fn prepare(&self, header: &mut Fields, parts: &mut Parts) -> Result<(), CoreError> {
        let script = &self.0;
        let Some(before_write) = &script.before_write else {
            return Ok(());
        };
        Python::attach(|py| {
            let (new_header, new_parts) = script
                .call_before_write(py, before_write, header, parts)
                .map_err(|e| script.error(py, &e))?;
            script.write_back(header, &script.header, &new_header)?;
            for (part, kinds) in &script.parts {
                let (Some(rows), Some(new_rows)) = (parts.get_mut(part), new_parts.get(part))
                else {
                    continue;
                };
                if rows.len() != new_rows.len() {
                    let reason = format!("число строк {part} изменилось: строки менять нельзя");
                    return Err(script.value_error(part, &reason));
                }
                for (row, new_row) in rows.iter_mut().zip(new_rows) {
                    script.write_back(row, kinds, new_row)?;
                }
            }
            Ok(())
        })
    }
}

/// Значение из Python: только `None`, `int` или `str` — так их отдаёт модуль `ostov`.
#[derive(FromPyObject)]
enum Plain {
    Int(i64),
    Text(String),
}

type PlainFields = BTreeMap<String, Option<Plain>>;
type Movements = Vec<(String, String, PlainFields)>;

impl Script {
    fn call_before_write(
        &self,
        py: Python<'_>,
        function: &Py<PyAny>,
        header: &Fields,
        parts: &Parts,
    ) -> PyResult<(PlainFields, BTreeMap<String, Vec<PlainFields>>)> {
        runtime(py)?
            .getattr("_before_write")?
            .call1((
                function,
                &self.name,
                self.schema(py)?,
                fields(py, header, &self.header)?,
                self.parts_dict(py, parts)?,
            ))?
            .extract()
    }

    fn call_on_post(
        &self,
        py: Python<'_>,
        function: &Py<PyAny>,
        doc: &Document,
        query: Bound<'_, PyCFunction>,
    ) -> PyResult<Movements> {
        let service = PyDict::new(py);
        service.set_item("id", doc.id)?;
        service.set_item("number", doc.number)?;
        service.set_item("posted", doc.posted)?;
        let registers = PyDict::new(py);
        for (name, dimensions, resources) in &self.registers {
            registers.set_item(
                name,
                (kinds_dict(py, dimensions)?, kinds_dict(py, resources)?),
            )?;
        }
        runtime(py)?
            .getattr("_on_post")?
            .call1((
                function,
                &self.name,
                self.schema(py)?,
                fields(py, &doc.header, &self.header)?,
                self.parts_dict(py, &doc.parts)?,
                service,
                registers,
                PyList::new(py, &self.allowed)?,
                query,
            ))?
            .extract()
    }

    fn schema<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let parts = PyDict::new(py);
        for (part, kinds) in &self.parts {
            parts.set_item(part, kinds_dict(py, kinds)?)?;
        }
        let schema = PyDict::new(py);
        schema.set_item("header", kinds_dict(py, &self.header)?)?;
        schema.set_item("parts", parts)?;
        Ok(schema)
    }

    fn parts_dict<'py>(&self, py: Python<'py>, parts: &Parts) -> PyResult<Bound<'py, PyDict>> {
        let result = PyDict::new(py);
        for (part, kinds) in &self.parts {
            let rows = PyList::empty(py);
            for row in parts.get(part).into_iter().flatten() {
                rows.append(fields(py, row, kinds)?)?;
            }
            result.set_item(part, rows)?;
        }
        Ok(result)
    }

    fn write_back(
        &self,
        target: &mut Fields,
        kinds: &Kinds,
        values: &PlainFields,
    ) -> Result<(), CoreError> {
        for (field, kind) in kinds {
            if let Some(value) = values.get(field) {
                let value =
                    from_plain(*kind, value.as_ref()).map_err(|e| self.value_error(field, &e))?;
                target.insert(field.clone(), value);
            }
        }
        Ok(())
    }

    fn movement(
        &self,
        register: &str,
        kind: &str,
        values: &PlainFields,
    ) -> Result<Movement, CoreError> {
        let kind = match kind {
            "receipt" => MovementKind::Receipt,
            "issue" => MovementKind::Issue,
            other => return Err(self.value_error(register, &format!("вид движения {other}"))),
        };
        let Some((_, dimensions, resources)) = self.registers.iter().find(|r| r.0 == register)
        else {
            return Err(self.value_error(register, "нет такого регистра"));
        };
        let mut movement = Movement {
            kind,
            dims: Fields::new(),
            resources: BTreeMap::new(),
        };
        self.write_back(&mut movement.dims, dimensions, values)?;
        for (field, _) in resources {
            let value = values.get(field).and_then(Option::as_ref);
            match from_plain(FieldKind::Decimal { scale: 0 }, value) {
                Ok(FieldValue::Decimal(amount)) => {
                    movement.resources.insert(field.clone(), amount);
                }
                Ok(_) => return Err(self.value_error(field, "ресурс не указан")),
                Err(e) => return Err(self.value_error(field, &e)),
            }
        }
        Ok(movement)
    }

    /// `ostov.UserError` — текст пользователю как есть; остальное — ошибка логики с трассировкой.
    fn error(&self, py: Python<'_>, error: &PyErr) -> CoreError {
        let described = runtime(py).and_then(|rt| {
            rt.getattr("_describe")?
                .call1((error.value(py),))?
                .extract::<(bool, String, String, String)>()
        });
        match described {
            Ok((true, _, text, _)) => CoreError::Invalid(text),
            Ok((false, kind, text, trace)) => CoreError::Script {
                message: format!("Ошибка в логике документа «{}»: {kind}: {text}", self.title),
                details: trace,
            },
            Err(_) => CoreError::Script {
                message: format!("Ошибка в логике документа «{}»: {error}", self.title),
                details: String::new(),
            },
        }
    }

    fn value_error(&self, field: &str, reason: &str) -> CoreError {
        CoreError::Script {
            message: format!(
                "Ошибка в логике документа «{}»: {field}: {reason}",
                self.title
            ),
            details: String::new(),
        }
    }
}

fn kind_code(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Int => "int",
        FieldKind::Text => "text",
        FieldKind::Decimal { .. } => "decimal",
        FieldKind::Date => "date",
        FieldKind::Ref => "ref",
    }
}

fn kinds_dict<'py>(py: Python<'py>, kinds: &Kinds) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (name, kind) in kinds {
        dict.set_item(name, kind_code(*kind))?;
    }
    Ok(dict)
}

/// Поля для Python: `None`, `int` или `str` (Decimal и дата — текстом).
fn fields<'py>(py: Python<'py>, values: &Fields, kinds: &Kinds) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (name, _) in kinds {
        dict.set_item(name, plain(py, values.get(name))?)?;
    }
    Ok(dict)
}

fn plain<'py>(py: Python<'py>, value: Option<&FieldValue>) -> PyResult<Bound<'py, PyAny>> {
    match value {
        None | Some(FieldValue::Null) => Ok(py.None().into_bound(py)),
        Some(FieldValue::Int(v) | FieldValue::Ref(v)) => v.into_bound_py_any(py),
        Some(FieldValue::Text(v) | FieldValue::Date(v)) => v.into_bound_py_any(py),
        Some(FieldValue::Decimal(v)) => v.to_string().into_bound_py_any(py),
    }
}

/// `ctx.query(sql, [(имя, вид, значение)])` → `([(колонка, вид)], строки)`.
fn query_function(py: Python<'_>, link: Link) -> PyResult<Bound<'_, PyCFunction>> {
    PyCFunction::new_closure(
        py,
        Some(c"query"),
        None,
        move |args: &Bound<'_, PyTuple>, _: Option<&Bound<'_, PyDict>>| -> PyResult<Py<PyAny>> {
            let py = args.py();
            let (sql, raw): (String, Vec<(String, String, Option<Plain>)>) = args.extract()?;
            let params = raw
                .into_iter()
                .map(|(name, kind, value)| Ok((name.clone(), param(py, &name, &kind, value)?)))
                .collect::<PyResult<_>>()?;
            let sender = link.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let Some(sender) = sender else {
                return Err(raise(
                    py,
                    "QueryError",
                    "ctx.query доступен только во время on_post",
                ));
            };
            let (answer, reply) = mpsc::channel();
            let found = match sender.send((sql, params, answer)) {
                Ok(()) => py.detach(move || reply.recv().ok()),
                Err(_) => None,
            };
            match found {
                Some(Ok(rows)) => rows_to_py(py, &rows),
                Some(Err(e @ CoreError::Query(SqlError::NoRight { .. }))) => {
                    Err(raise(py, "UserError", &e.to_string()))
                }
                // Текст базы бывает английским и техническим: он уходит в трассировку.
                Some(Err(e @ CoreError::Store(_))) => Err(raise_noted(
                    py,
                    "ошибка базы данных при выполнении запроса",
                    &e.to_string(),
                )),
                Some(Err(e)) => Err(raise(py, "QueryError", &e.to_string())),
                None => Err(raise(py, "QueryError", "ядро не ответило на запрос")),
            }
        },
    )
}

fn param(py: Python<'_>, name: &str, kind: &str, value: Option<Plain>) -> PyResult<Param> {
    match (kind, value) {
        (_, None) => Ok(Param::Null),
        ("int", Some(Plain::Int(v))) => Ok(Param::Int(v)),
        ("text", Some(Plain::Text(v))) => Ok(Param::Text(v)),
        ("decimal", Some(Plain::Text(v))) => match v.parse::<Decimal>() {
            Ok(d) => Ok(Param::Decimal {
                units: d.units(),
                scale: d.scale(),
            }),
            Err(e) => Err(raise(py, "QueryError", &format!("параметр {name}: {e}"))),
        },
        _ => Err(raise(
            py,
            "QueryError",
            &format!("параметр {name}: неверное значение"),
        )),
    }
}

fn rows_to_py(py: Python<'_>, found: &QueryRows) -> PyResult<Py<PyAny>> {
    let columns = PyList::empty(py);
    for column in &found.columns {
        columns.append((&column.name, column.kind.map_or("int", kind_code)))?;
    }
    let rows = PyList::empty(py);
    for row in &found.rows {
        let values = PyList::empty(py);
        for (column, value) in found.columns.iter().zip(row) {
            match value {
                FieldValue::Int(n) if column.flag => values.append(*n != 0)?,
                _ => values.append(plain(py, Some(value))?)?,
            }
        }
        rows.append(values)?;
    }
    (columns, rows).into_py_any(py)
}

/// Исключение модуля `ostov` по имени класса.
fn raise(py: Python<'_>, class: &str, message: &str) -> PyErr {
    runtime(py)
        .and_then(|rt| rt.getattr(class)?.call1((message,)))
        .map_or_else(|e| e, PyErr::from_value)
}

fn raise_noted(py: Python<'_>, message: &str, note: &str) -> PyErr {
    runtime(py)
        .and_then(|rt| {
            let error = rt.getattr("QueryError")?.call1((message,))?;
            error.call_method1("add_note", (note,))?;
            Ok(error)
        })
        .map_or_else(|e| e, PyErr::from_value)
}

fn from_plain(kind: FieldKind, value: Option<&Plain>) -> Result<FieldValue, String> {
    let Some(value) = value else {
        return Ok(FieldValue::Null);
    };
    match (kind, value) {
        (FieldKind::Int, Plain::Int(v)) => Ok(FieldValue::Int(*v)),
        (FieldKind::Ref, Plain::Int(v)) => Ok(FieldValue::Ref(*v)),
        (FieldKind::Text, Plain::Text(v)) => Ok(FieldValue::Text(v.clone())),
        (FieldKind::Date, Plain::Text(v)) => Ok(FieldValue::Date(v.clone())),
        (FieldKind::Decimal { .. }, Plain::Text(v)) => v
            .parse::<Decimal>()
            .map(FieldValue::Decimal)
            .map_err(|e| e.to_string()),
        (FieldKind::Decimal { .. }, Plain::Int(v)) => Decimal::new(*v, 0)
            .map(FieldValue::Decimal)
            .map_err(|e| e.to_string()),
        _ => Err(format!("значение не подходит к типу {}", kind_code(kind))),
    }
}
