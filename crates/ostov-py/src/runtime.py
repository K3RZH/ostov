"""Модуль ostov для конфигураций: UserError, документ и движения.

Встроен в ostov-py и кладётся в sys.modules до загрузки .py документов. Ядро передаёт сюда
значения только как None, int и str; Decimal и даты собираются здесь по типам полей.
"""

import datetime
import traceback
from decimal import Decimal

__all__ = ["QueryError", "UserError"]


class UserError(Exception):
    """Ошибка для пользователя: он увидит ровно этот текст."""


class QueryError(Exception):
    """ctx.query не прошёл: запрос не по правилам ostov-sql или ошибка базы."""


_EXPECTED = {
    "int": "int",
    "ref": "int (id)",
    "text": "str",
    "decimal": "Decimal или int",
    "date": "datetime.date",
}


def _check(owner, field, kind, value):
    """Значение поля из Python: float запрещён (СТ-56), остальное — по типу поля."""
    if value is None:
        return None
    if isinstance(value, float):
        raise TypeError(f"{owner}.{field}: числа только Decimal или int, а не float")
    if isinstance(value, bool):
        ok = False
    elif kind in ("int", "ref"):
        ok = isinstance(value, int)
    elif kind == "decimal":
        ok = isinstance(value, (Decimal, int))
    elif kind == "text":
        ok = isinstance(value, str)
    else:
        ok = isinstance(value, datetime.date) and not isinstance(value, datetime.datetime)
    if not ok:
        raise TypeError(
            f"{owner}.{field}: ожидается {_EXPECTED[kind]}, а не {type(value).__name__}"
        )
    return value


def _from_core(kind, value):
    if value is None:
        return None
    if kind == "decimal":
        return Decimal(value)
    if kind == "date":
        # Неверную дату отклонит ядро; скрипт видит её как есть, строкой.
        try:
            return datetime.date.fromisoformat(value) if len(value) == 10 else value
        except ValueError:
            return value
    return value


def _to_core(kind, value):
    if value is None:
        return None
    if kind == "decimal":
        value = Decimal(value)
        if not value.is_finite():
            raise ValueError(f"не число: {value}")
        return format(value, "f")
    if kind == "date":
        return value.isoformat()
    return value


class _Record:
    """Поля атрибутами. В on_post запись недоступна, новых полей не бывает."""

    __slots__ = ("_owner", "_kinds", "_values", "_writable", "_changed")

    def __init__(self, owner, kinds, values, writable):
        object.__setattr__(self, "_owner", owner)
        object.__setattr__(self, "_kinds", kinds)
        object.__setattr__(self, "_values", values)
        object.__setattr__(self, "_writable", writable)
        object.__setattr__(self, "_changed", set())

    def __getattr__(self, name):
        try:
            return self._values[name]
        except KeyError:
            raise AttributeError(f"{self._owner}: нет поля {name}") from None

    def __setattr__(self, name, value):
        if not self._writable:
            raise AttributeError(f"{self._owner}: в on_post поля только для чтения")
        if name not in self._kinds:
            raise AttributeError(f"{self._owner}: нет поля {name}, которое можно менять")
        self._values[name] = _check(self._owner, name, self._kinds[name], value)
        self._changed.add(name)

    def __repr__(self):
        return f"<{self._owner} {self._values!r}>"


def _record(owner, kinds, raw, writable):
    values = {name: _from_core(kind, raw.get(name)) for name, kind in kinds.items()}
    return _Record(owner, kinds, values, writable)


def _document(name, schema, header, parts, service, writable):
    kinds = schema["header"]
    values = {field: _from_core(kind, header.get(field)) for field, kind in kinds.items()}
    for part, part_kinds in schema["parts"].items():
        rows = parts.get(part, [])
        # Кортеж: добавлять и удалять строки в R1 нельзя.
        values[part] = tuple(
            _record(f"{name}.{part}", part_kinds, row, writable) for row in rows
        )
    values.update(service)
    return _Record(name, kinds, values, writable)


def _dump(record, kinds):
    """Только поля, которым скрипт присвоил значение: остальное ядро проверит само как ввод."""
    return {
        field: _to_core(kind, record._values.get(field))
        for field, kind in kinds.items()
        if field in record._changed
    }


def _param(name, value):
    """Параметр запроса для ядра: (имя, вид, значение)."""
    if value is None:
        return name, "null", None
    if isinstance(value, bool):
        return name, "int", int(value)
    if isinstance(value, int):
        return name, "int", value
    if isinstance(value, Decimal):
        if not value.is_finite():
            raise ValueError(f"параметр {name}: не число: {value}")
        return name, "decimal", format(value, "f")
    if isinstance(value, str):
        return name, "text", value
    if isinstance(value, datetime.date) and not isinstance(value, datetime.datetime):
        return name, "text", value.isoformat()
    raise TypeError(
        f"параметр {name}: можно int, Decimal, str, datetime.date или None, "
        f"а не {type(value).__name__}"
    )


class _Ctx:
    """Движения документа: ctx.receipt(...) и ctx.issue(...); чтение: ctx.query(...)."""

    def __init__(self, document, registers, allowed, query):
        self._document = document
        self._registers = registers
        self._allowed = allowed
        self._query = query
        self._moves = []

    def query(self, sql, /, **params):
        """Строки SELECT через платформу, не больше 1000: row.поле.

        Параметры — :имя в тексте и именованные аргументы. Деньги приходят Decimal,
        даты — datetime.date, ссылки — id.
        """
        if self._query is None:
            raise QueryError("ctx.query доступен только в on_post")
        columns, rows = self._query(sql, [_param(k, v) for k, v in params.items()])
        kinds = dict(columns)
        return [_record("запрос", kinds, dict(zip(kinds, row)), False) for row in rows]

    def receipt(self, register, /, **values):
        """Приход по регистру; ключи — его измерения и ресурсы."""
        self._move("receipt", register, values)

    def issue(self, register, /, **values):
        """Расход по регистру; ключи — его измерения и ресурсы."""
        self._move("issue", register, values)

    def _move(self, kind, register, values):
        if register not in self._registers:
            raise ValueError(f"нет регистра «{register}»")
        if register not in self._allowed:
            raise ValueError(
                f"регистра «{register}» нет в registers документа {self._document}"
            )
        dimensions, resources = self._registers[register]
        expected = dimensions.keys() | resources.keys()
        extra = sorted(values.keys() - expected)
        missing = sorted(expected - values.keys())
        if extra:
            raise TypeError(f"{register}: лишние ключи {', '.join(extra)}")
        if missing:
            raise TypeError(f"{register}: не хватает ключей {', '.join(missing)}")
        row = {}
        for field, field_kind in {**dimensions, **resources}.items():
            value = _check(register, field, field_kind, values[field])
            if field in resources and value is None:
                raise ValueError(f"{register}: ресурс {field} не указан")
            row[field] = _to_core(field_kind, value)
        self._moves.append((register, kind, row))


def _before_write(func, name, schema, header, parts):
    doc = _document(name, schema, header, parts, {"id": None, "number": None, "posted": False}, True)
    func(doc, None)
    rows = {
        part: [_dump(row, kinds) for row in doc._values[part]]
        for part, kinds in schema["parts"].items()
    }
    return _dump(doc, schema["header"]), rows


def _on_post(func, name, schema, header, parts, service, registers, allowed, query):
    doc = _document(name, schema, header, parts, service, False)
    ctx = _Ctx(name, registers, allowed, query)
    func(doc, ctx)
    return ctx._moves


def _load_error(error, filename):
    """(строка, причина) ошибки при загрузке модуля документа."""
    if isinstance(error, SyntaxError):
        return error.lineno, f"синтаксическая ошибка: {error.msg}"
    line = None
    for frame in traceback.extract_tb(error.__traceback__):
        if frame.filename == filename:
            line = frame.lineno
    return line, f"{type(error).__name__}: {error}"


def _describe(error):
    """(это UserError?, тип, текст, трассировка) — для перевода в ошибку ядра."""
    trace = "".join(traceback.format_exception(error))
    return isinstance(error, UserError), type(error).__name__, str(error), trace
