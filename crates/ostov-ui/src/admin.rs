//! Данные экранов «Журнал действий» и «Закрытие периода» без интерфейса и без ядра. Что и как
//! просить у ядра, решает `admin_ui.rs`; здесь страницы журнала и тексты, которые можно проверить
//! обычными тестами.

use ostov_api::JournalEntry;

/// Записей в одной странице журнала. Страницы идут по ключу последней записи, не по OFFSET
/// (ADR-022): новые записи, пока читают, не сдвигают следующую страницу.
pub const JOURNAL_PAGE: u32 = 50;

/// Записи журнала, которые уже показаны, и признак, что за ними есть ещё.
#[derive(Debug, Default)]
pub struct Journal {
    entries: Vec<JournalEntry>,
    more: bool,
}

impl Journal {
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    pub fn has_more(&self) -> bool {
        self.more
    }

    /// Ключ следующей страницы (`id` последней записи) или `None`, если записей больше нет.
    pub fn next_key(&self) -> Option<String> {
        if !self.more {
            return None;
        }
        self.entries.last().map(|entry| entry.id.to_string())
    }

    /// Первая страница заменяет всё, что было показано («Обновить»).
    pub fn replace(&mut self, page: Vec<JournalEntry>) {
        self.more = is_full(&page);
        self.entries = page;
    }

    /// Следующая страница дописывается к показанным.
    pub fn append(&mut self, page: Vec<JournalEntry>) {
        self.more = is_full(&page);
        self.entries.extend(page);
    }
}

/// Ядро отдаёт ровно `JOURNAL_PAGE` записей, пока есть что читать дальше. Если записей меньше,
/// это последняя страница; если их столько, сколько просили, следующая может оказаться пустой, и
/// тогда «Ещё» исчезнет после нажатия.
fn is_full(page: &[JournalEntry]) -> bool {
    page.len() >= usize::try_from(JOURNAL_PAGE).unwrap_or(usize::MAX)
}

/// Время записи `2026-11-08T12:34:56Z` (UTC) в «08.11.2026 12:34:56». Что не похоже на время
/// ядра, возвращается как есть: лучше показать сырой текст, чем спрятать запись.
pub fn format_time(utc: &str) -> String {
    let digits =
        |text: &str, len: usize| text.len() == len && text.bytes().all(|b| b.is_ascii_digit());
    let shown = || -> Option<String> {
        let (date, time) = utc.strip_suffix('Z').unwrap_or(utc).split_once('T')?;
        let (year, rest) = date.split_once('-')?;
        let (month, day) = rest.split_once('-')?;
        let mut clock = time.split(':');
        let (hour, minute, second) = (clock.next()?, clock.next()?, clock.next()?);
        let valid = digits(year, 4)
            && digits(month, 2)
            && digits(day, 2)
            && digits(hour, 2)
            && digits(minute, 2)
            && digits(second, 2)
            && clock.next().is_none();
        valid.then(|| format!("{day}.{month}.{year} {hour}:{minute}:{second}"))
    };
    shown().unwrap_or_else(|| utc.to_owned())
}

/// Кто сделал: в базе без пользователей вход не нужен, и ядро пишет пустой логин.
pub fn user_label(login: &str) -> &str {
    if login.is_empty() {
        "(без входа)"
    } else {
        login
    }
}

/// Год, месяц и день из даты ядра `ГГГГ-ММ-ДД`.
pub fn parse_iso_date(iso: &str) -> Option<(i32, i32, i32)> {
    let mut parts = iso.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((year, month, day))
}

/// Дата для ядра `ГГГГ-ММ-ДД` из частей поля даты. Существует ли такой день, проверяет ядро: его
/// ошибка и покажется пользователю.
pub fn iso_date(year: i32, month: i32, day: i32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// Что известно о закрытии периода.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Closing {
    /// Ядро ещё не ответило.
    #[default]
    Unknown,
    Open,
    /// Закрыт по эту дату включительно (`ГГГГ-ММ-ДД`).
    Through(String),
}

impl Closing {
    /// Ответ ядра `closing_date`.
    pub fn new(date: Option<String>) -> Self {
        date.map_or(Self::Open, Self::Through)
    }

    pub fn is_known(&self) -> bool {
        *self != Self::Unknown
    }

    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Through(_))
    }

    pub fn text(&self) -> String {
        match self {
            Self::Unknown => "Загрузка…".to_owned(),
            Self::Open => "Период не закрыт".to_owned(),
            Self::Through(date) => format!("Период закрыт по {}", ru_date(date)),
        }
    }

    /// Дата закрытия по частям (год, месяц, день), чтобы поставить её в поле даты.
    pub fn date(&self) -> Option<(i32, i32, i32)> {
        match self {
            Self::Through(date) => parse_iso_date(date),
            Self::Unknown | Self::Open => None,
        }
    }
}

/// `2026-01-31` в «31.01.2026»; непонятный текст остаётся как есть.
fn ru_date(iso: &str) -> String {
    parse_iso_date(iso).map_or_else(
        || iso.to_owned(),
        |(year, month, day)| format!("{day:02}.{month:02}.{year:04}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64) -> JournalEntry {
        JournalEntry {
            id,
            when: "2026-11-08T12:34:56Z".into(),
            user: "admin".into(),
            object: String::new(),
            action: "Вход".into(),
            details: String::new(),
        }
    }

    fn page(from: i64, count: i64) -> Vec<JournalEntry> {
        (0..count).map(|i| entry(from - i)).collect()
    }

    #[test]
    fn time_is_shown_in_russian_order() {
        assert_eq!(format_time("2026-11-08T12:34:56Z"), "08.11.2026 12:34:56");
        assert_eq!(format_time("2026-01-02T03:04:05"), "02.01.2026 03:04:05");
    }

    #[test]
    fn entry_without_login_is_explained() {
        assert_eq!(user_label(""), "(без входа)");
        assert_eq!(user_label("admin"), "admin");
    }

    #[test]
    fn unexpected_time_is_shown_as_is() {
        for text in [
            "",
            "вчера",
            "2026-11-08",
            "2026-11-08T12:34Z",
            "26-11-08T12:34:56Z",
        ] {
            assert_eq!(format_time(text), text);
        }
    }

    #[test]
    fn full_page_offers_more_and_gives_the_key() {
        let mut journal = Journal::default();
        assert_eq!(journal.next_key(), None);
        journal.replace(page(100, i64::from(JOURNAL_PAGE)));
        assert!(journal.has_more());
        // Ключ это id последней, самой старой из показанных записей.
        assert_eq!(journal.next_key().as_deref(), Some("51"));
    }

    #[test]
    fn short_page_is_the_last_one() {
        let mut journal = Journal::default();
        journal.replace(page(100, i64::from(JOURNAL_PAGE)));
        journal.append(page(50, 3));
        assert!(!journal.has_more());
        assert_eq!(journal.next_key(), None);
        assert_eq!(
            journal.entries().len(),
            usize::try_from(JOURNAL_PAGE).unwrap_or(0) + 3
        );
    }

    #[test]
    fn refresh_replaces_what_was_shown() {
        let mut journal = Journal::default();
        journal.replace(page(100, i64::from(JOURNAL_PAGE)));
        journal.append(page(50, i64::from(JOURNAL_PAGE)));
        journal.replace(page(120, 2));
        assert_eq!(journal.entries().len(), 2);
        assert!(!journal.has_more());
    }

    #[test]
    fn closing_texts_follow_the_state() {
        assert_eq!(Closing::default().text(), "Загрузка…");
        assert!(!Closing::default().is_known());
        assert_eq!(Closing::new(None).text(), "Период не закрыт");
        let closed = Closing::new(Some("2026-01-31".into()));
        assert_eq!(closed.text(), "Период закрыт по 31.01.2026");
        assert!(closed.is_closed() && closed.is_known());
        assert!(!Closing::new(None).is_closed());
    }

    #[test]
    fn closing_date_goes_to_the_field_and_back() {
        let closed = Closing::new(Some("2026-02-09".into()));
        assert_eq!(closed.date(), Some((2026, 2, 9)));
        assert_eq!(Closing::new(None).date(), None);
        let (year, month, day) = closed.date().unwrap_or_default();
        assert_eq!(iso_date(year, month, day), "2026-02-09");
        // Нули впереди нужны: ядро принимает только ГГГГ-ММ-ДД.
        assert_eq!(iso_date(2026, 1, 5), "2026-01-05");
    }
}
