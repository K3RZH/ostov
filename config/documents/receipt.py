"""Поступление: суммы строк и документа, приход на склад."""

from decimal import ROUND_HALF_UP, Decimal
from typing import TYPE_CHECKING

import ostov

if TYPE_CHECKING:
    from _stubs.receipt import Receipt
    from ostov import Ctx

CENT = Decimal("0.01")


def before_write(doc: "Receipt", ctx: None) -> None:
    """Сумма строки — количество × цена до копеек «от нуля», сумма документа — их итог."""
    total = Decimal("0.00")
    for row in doc.lines:
        if row.qty is None or row.price is None:
            row.amount = None
            continue
        row.amount = (row.qty * row.price).quantize(CENT, ROUND_HALF_UP)
        total += row.amount
    doc.amount = total


def on_post(doc: "Receipt", ctx: "Ctx") -> None:
    """Каждая строка — приход по складу документа и товару строки."""
    for row in doc.lines:
        if row.qty is None:
            raise ostov.UserError("в строке не указано количество")
        ctx.receipt("stock", warehouse=doc.warehouse, goods=row.goods, qty=row.qty)
