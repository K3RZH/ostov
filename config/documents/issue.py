"""Расход: списание со склада."""

import ostov


def on_post(doc, ctx):
    """Каждая строка — расход по складу документа и товару строки."""
    for row in doc.lines:
        if row.qty is None:
            raise ostov.UserError("в строке не указано количество")
        ctx.issue("stock", warehouse=doc.warehouse, goods=row.goods, qty=row.qty)
