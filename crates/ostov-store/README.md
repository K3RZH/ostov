# ostov-store

Слой: Layer0.

Слой хранения: драйверы и диалекты СУБД (SQLite, PostgreSQL), тип Decimal для денег и количеств (целое с масштабом, без float), миграции схемы.

Статус: есть `Decimal`, трейты `Driver` и `Dialect`, драйвер SQLite (фича `sqlite`, включена по умолчанию). PostgreSQL — следующим.
