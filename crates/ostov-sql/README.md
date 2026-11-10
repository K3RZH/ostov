# ostov-sql

Слой: Надстройки.

SQL через платформу (СТ-13, СТ-14, ADR-004): текст запроса из конфигурации на входе, SQL под диалект с параметрами на выходе. Базу крейт не трогает, выполняет запрос ядро.

```rust
let p = ostov_sql::prepare(
    "SELECT s.name, SUM(r.amount) AS total FROM receipt r \
     JOIN suppliers s ON s.id = r.supplier WHERE r.date >= :from GROUP BY s.name",
    &params,              // BTreeMap<String, ostov_sql::Param>
    &config,              // ostov_model::Config
    &|object| can_read(object),
    db.dialect(),
)?;
let rows = db.query(&p.sql, &p.params)?; // p.columns: имена и типы колонок
```

- Можно: один `SELECT`, `FROM`, `JOIN` (`INNER`, `LEFT`) с `ON`, `WHERE`, `GROUP BY`, `HAVING`, `ORDER BY`, `LIMIT`; функции `COUNT`, `SUM`, `MIN`, `MAX`, `COALESCE`, `LOWER`; сравнения, `AND`/`OR`/`NOT`, `IN (список)`, `BETWEEN`, `IS NULL`, `LIKE`, `+ - *`; целые числа и строки в одинарных кавычках; параметры `:имя`.
- Нельзя: всё остальное, в том числе `SELECT *`, `DISTINCT`, `OFFSET`, `WITH`, `UNION`, подзапросы, имена в кавычках, дробные числа в тексте.
- Имена: `goods`, `receipt`, `stock` — как в метаданных, табличная часть — `receipt.lines`.
- `LIMIT` не больше 1000, без `LIMIT` ставится 1000.
- Выполняют запрос ядро (`Engine::query`) и правила на Python (`ctx.query`).
- Права: `can_read` для справочников и документов, табличная часть по праву документа, регистры без проверки.
- Суммы хранятся целым (копейки и т. п.): сравнивать и складывать их с целым числом в тексте или целым полем нельзя, кроме `0`. Значение передавайте параметром: `Param::Decimal` или `Param::Int` рядом с денежным полем переводится в его единицы (`price > :min`, `min = 2,5` → `250`), лишние знаки после запятой — ошибка. Умножать на целое можно, при умножении масштабы складываются.
- `LIKE` в SQLite не различает регистр латиницы, в PostgreSQL различает. Для поиска пишите `LOWER(x) LIKE LOWER(:p)`.
- Ограничения: запрос до 64 КБ и до 2000 частей (токенов), выражение не глубже 64 уровней. Длинные цепочки `OR` замените на `IN (…)`.
