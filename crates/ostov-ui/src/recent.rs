//! Недавние базы этого компьютера: `~/.config/ostov/recent.conf`, строка на базу,
//! `local<TAB>имя<TAB>путь` или `server<TAB>имя<TAB>адрес`. Список живёт вне базы: он нужен
//! до того, как база открыта, поэтому ядро про него не знает.

use crate::RecentDb;

/// Сколько баз помнить: старые уходят из списка, файлы баз не трогаем.
const MAX_RECENT: usize = 20;
const FILE: &str = "recent.conf";

/// Разбирает файл списка; строки, которые не удалось разобрать, пропускает.
pub fn parse(text: &str) -> Vec<RecentDb> {
    text.lines()
        .filter_map(parse_line)
        .take(MAX_RECENT)
        .collect()
}

fn parse_line(line: &str) -> Option<RecentDb> {
    let mut parts = line.split('\t');
    let server = match parts.next()? {
        "local" => false,
        "server" => true,
        _ => return None,
    };
    let (name, path) = (parts.next()?, parts.next()?);
    if name.is_empty() || path.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(RecentDb {
        name: name.into(),
        path: path.into(),
        when: "".into(),
        server,
        protection: 0,
    })
}

pub fn to_text(rows: &[RecentDb]) -> String {
    rows.iter()
        .take(MAX_RECENT)
        .map(|db| {
            let kind = if db.server { "server" } else { "local" };
            format!("{kind}\t{}\t{}\n", clean(&db.name), clean(&db.path))
        })
        .collect()
}

/// Табуляция и перевод строки в имени сломали бы формат файла.
fn clean(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

/// Ставит базу первой; та же база (тот же путь или адрес) второй раз не попадает.
pub fn push_front(rows: impl IntoIterator<Item = RecentDb>, db: RecentDb) -> Vec<RecentDb> {
    let rest: Vec<RecentDb> = rows
        .into_iter()
        .filter(|row| !(row.server == db.server && row.path == db.path))
        .collect();
    let mut result = Vec::with_capacity(rest.len() + 1);
    result.push(db);
    result.extend(rest);
    result.truncate(MAX_RECENT);
    result
}

/// Читает список; файла нет или он не читается: список пуст.
pub fn load() -> Vec<RecentDb> {
    crate::settings::config_file(FILE)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Сохраняет список.
///
/// # Errors
/// Текст для пользователя, если папку настроек не найти или файл не записать.
pub fn save(rows: &[RecentDb]) -> Result<(), String> {
    let path = crate::settings::config_file(FILE)
        .ok_or_else(|| "Не найдена папка настроек: список баз не сохранён.".to_owned())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Список баз не сохранён: {error}."))?;
    }
    std::fs::write(&path, to_text(rows))
        .map_err(|error| format!("Список баз не сохранён: {error}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, path: &str) -> RecentDb {
        RecentDb {
            name: name.into(),
            path: path.into(),
            when: "".into(),
            server: false,
            protection: 0,
        }
    }

    #[test]
    fn list_survives_save_and_load() {
        let rows = vec![
            local("Склад.sqlite", "~/Ostov/Склад.sqlite"),
            RecentDb {
                server: true,
                ..local("Кафе", "server.local:7700")
            },
        ];
        assert_eq!(parse(&to_text(&rows)), rows);
    }

    #[test]
    fn broken_lines_are_skipped() {
        let rows =
            parse("local\ta\t/a\nмусор\nserver\t\taddr\nlocal\tb\t/b\textra\nlocal\tc\t/c\n");
        let names: Vec<_> = rows.iter().map(|db| db.name.to_string()).collect();
        assert_eq!(names, ["a", "c"]);
    }

    #[test]
    fn tab_in_name_does_not_break_file() {
        let rows = vec![local("a\tb", "/x")];
        assert_eq!(parse(&to_text(&rows))[0].name, "a b");
    }

    #[test]
    fn same_base_moves_to_top_once() {
        let rows = vec![local("a", "/a"), local("b", "/b")];
        let rows = push_front(rows, local("b снова", "/b"));
        let paths: Vec<_> = rows.iter().map(|db| db.path.to_string()).collect();
        assert_eq!(paths, ["/b", "/a"]);
    }
}
