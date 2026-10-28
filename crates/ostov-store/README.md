# ostov-store

Слой: Layer0.

Слой хранения: драйверы и диалекты СУБД (SQLite, PostgreSQL), тип Decimal для денег и количеств (целое с масштабом, без float), миграции схемы.

Статус: есть `Decimal`, трейты `Driver` и `Dialect`, драйверы в `src/drivers/`:
SQLite (фича `sqlite`, по умолчанию) и PostgreSQL (фича `postgres`).

Тесты PostgreSQL с живой базой запускаются, если задан `OSTOV_PG_URL`:

    OSTOV_PG_URL="host=localhost user=postgres password=postgres dbname=ostov_test" cargo test --all-features
