//! Описание схемы базы для внешних систем отчётности (СТ-58): какие таблицы и колонки
//! и как читать значения. Таблица — объект конфигурации, колонка — поле (ADR-025).

use std::fmt::Write;

use ostov_model::{Config, FieldDef, FieldKind};

/// Служебная колонка: имя, тип, что это.
type System = (&'static str, String, &'static str);

/// Markdown со всеми таблицами конфигурации и правилами чтения значений.
pub fn describe_schema(config: &Config) -> String {
    let mut out = format!(
        "# Схема базы «{}» {}\n\n\
         Таблица — объект конфигурации, колонка — поле. Таблицы `ostov_*` служебные, \
         для отчётов не нужны.\n\n\
         **Как читать значения:**\n\
         - `decimal(n)` — целое число единиц: значение = число / 10^n \
         (`12345` при `decimal(2)` — 123,45);\n\
         - `date` — текст `ГГГГ-ММ-ДД`;\n\
         - `ref(таблица)` — `id` строки в этой таблице;\n\
         - `posted`, `deleted` — 1 да, 0 нет;\n\
         - в регистре `kind` — 1 приход, −1 расход: остаток = сумма `kind × ресурс`, \
         текущие остатки уже посчитаны в `<регистр>__totals`.\n",
        config.title, config.version
    );
    let int = || "int".to_owned();
    for c in &config.catalogs {
        let system = [
            ("id", int(), "ключ"),
            ("version", int(), "версия записи"),
            ("deleted", int(), "помечен на удаление"),
        ];
        table(&mut out, &c.name, &c.title, &system, &c.fields);
    }
    for d in &config.documents {
        let system = [
            ("id", int(), "ключ"),
            ("number", int(), "номер документа"),
            ("version", int(), "версия записи"),
            ("posted", int(), "проведён"),
            ("deleted", int(), "помечен на удаление"),
        ];
        table(&mut out, &d.name, &d.title, &system, &d.fields);
        for p in &d.table_parts {
            let system = [
                ("id", int(), "ключ"),
                ("doc_id", format!("ref({})", d.name), "документ"),
                ("line_no", int(), "номер строки"),
            ];
            let name = format!("{}__{}", d.name, p.name);
            let title = format!("{}: {}", d.title, p.title);
            table(&mut out, &name, &title, &system, &p.fields);
        }
    }
    for r in &config.registers {
        let fields: Vec<FieldDef> = r.dimensions.iter().chain(&r.resources).cloned().collect();
        let system = [
            ("id", int(), "ключ"),
            (
                "recorder",
                "text".to_owned(),
                "таблица документа-регистратора",
            ),
            ("recorder_id", int(), "id документа-регистратора"),
            ("kind", int(), "1 приход, −1 расход"),
            ("period", "date".to_owned(), "дата документа"),
        ];
        let title = format!("{}: движения", r.title);
        table(&mut out, &r.name, &title, &system, &fields);
        let name = format!("{}__totals", r.name);
        let title = format!("{}: текущие остатки", r.title);
        table(&mut out, &name, &title, &[], &fields);
    }
    out.push_str(
        "\n## Доступ только на чтение (PostgreSQL)\n\n\
         Внешней системе отчётности — своя учётная запись без права записи:\n\n\
         ```sql\n\
         CREATE ROLE reports LOGIN PASSWORD '…';\n\
         GRANT CONNECT ON DATABASE <база> TO reports;\n\
         GRANT USAGE ON SCHEMA <схема> TO reports;\n\
         GRANT SELECT ON ALL TABLES IN SCHEMA <схема> TO reports;\n\
         ALTER DEFAULT PRIVILEGES IN SCHEMA <схема> GRANT SELECT ON TABLES TO reports;\n\
         ```\n\n\
         Права ролей Ostov на такую учётную запись не действуют: давайте её только тем, кому \
         можно видеть все данные. Менять данные — только через Ostov.\n",
    );
    out
}

fn table(out: &mut String, name: &str, title: &str, system: &[System], fields: &[FieldDef]) {
    let _ = write!(
        out,
        "\n## `{name}` — {title}\n\n| Колонка | Тип | Что это |\n|---|---|---|\n"
    );
    for (column, kind, what) in system {
        let _ = writeln!(out, "| `{column}` | `{kind}` | {what} |");
    }
    for f in fields {
        let _ = writeln!(out, "| `{}` | `{}` | {} |", f.name, kind(f), f.title);
    }
}

fn kind(f: &FieldDef) -> String {
    match f.kind {
        FieldKind::Int => "int".to_owned(),
        FieldKind::Text => "text".to_owned(),
        FieldKind::Date => "date".to_owned(),
        FieldKind::Decimal { scale } => format!("decimal({scale})"),
        FieldKind::Ref => format!("ref({})", f.target.as_deref().unwrap_or("?")),
    }
}
