"""Расход: списание со склада."""

from typing import TYPE_CHECKING

import ostov

if TYPE_CHECKING:
    from _stubs.issue import Issue
    from ostov import Ctx


def on_post(doc: "Issue", ctx: "Ctx") -> None:
    """Каждая строка — расход по складу документа и товару строки."""
    for row in doc.lines:
        if row.qty is None:
            raise ostov.UserError("в строке не указано количество")
        ctx.issue("stock", warehouse=doc.warehouse, goods=row.goods, qty=row.qty)
