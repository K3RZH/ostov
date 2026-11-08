"""Проба ADR-018: функции, которые вызывает встроенный интерпретатор."""

import sys
from decimal import ROUND_HALF_UP, Decimal


def amount(qty: str, price: str) -> str:
    """Сумма строки с округлением до копеек «от нуля», как в ядре."""
    return str((Decimal(qty) * Decimal(price)).quantize(Decimal("0.01"), ROUND_HALF_UP))


def where_am_i() -> str:
    """Какой Python работает и откуда он взял стандартную библиотеку."""
    import _decimal  # C-реализация decimal из DLLs: без неё Decimal был бы медленным

    return f"Python {sys.version.split()[0]}, prefix={sys.prefix}, _decimal={_decimal.__file__}"
