"""Подсказки для модуля ostov: он встроен в Ostov, отдельного файла у него нет.

Классы документов и строк генерирует `ostov stubs` в config/_stubs/.
"""

import datetime
from decimal import Decimal
from typing import Protocol

Value = int | str | Decimal | datetime.date | None

class UserError(Exception):
    """Ошибка для пользователя: он увидит ровно этот текст."""

class Ctx(Protocol):
    """Движения документа в on_post. Ключи — измерения и ресурсы регистра, все обязательны."""

    def receipt(self, register: str, **values: Value) -> None:
        """Приход по регистру из registers документа."""

    def issue(self, register: str, **values: Value) -> None:
        """Расход по регистру из registers документа."""
