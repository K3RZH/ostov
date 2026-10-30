//! Данные списка «Поступления» и строки документа: типы интерфейса и форматирование.
//! Сами данные, порядок и поиск дают ядро (Engine API).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// Идентификатор в ядре (`Row::id`); у ещё не записанного документа 0.
    pub id: i64,
    pub number: u32,
    /// Дата как (год, месяц, день): так сортировка совпадает с календарной.
    pub date: (u16, u8, u8),
    pub supplier: String,
    pub warehouse: String,
    /// Сумма в копейках: деньги не храним дробным числом.
    pub amount: i64,
    pub posted: bool,
    /// Помечен на удаление: окончательно удаляется отдельным действием.
    pub deleted: bool,
}

/// Колонки списка в порядке показа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Number,
    Date,
    Supplier,
    Warehouse,
    Amount,
    Status,
}

pub const COLUMNS: [Column; 6] = [
    Column::Number,
    Column::Date,
    Column::Supplier,
    Column::Warehouse,
    Column::Amount,
    Column::Status,
];

impl Column {
    pub fn title(self) -> &'static str {
        match self {
            Self::Number => "№",
            Self::Date => "Дата",
            Self::Supplier => "Поставщик",
            Self::Warehouse => "Склад",
            Self::Amount => "Сумма",
            Self::Status => "Статус",
        }
    }
}

/// Сортировка списка по нескольким колонкам. Порядок ключей это приоритет: он такой, в каком
/// пользователь включал сортировку. У колонки три состояния: без сортировки, по возрастанию,
/// по убыванию (щелчок по заголовку переключает по кругу, меню заголовка задаёт прямо).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sorting {
    keys: Vec<(usize, bool)>,
}

impl Sorting {
    /// Ключи по приоритету: колонка и признак «по убыванию».
    pub fn keys(&self) -> &[(usize, bool)] {
        &self.keys
    }

    fn position(&self, column: usize) -> Option<usize> {
        self.keys.iter().position(|(c, _)| *c == column)
    }

    /// Состояние колонки: 0 нет, 1 по возрастанию, 2 по убыванию.
    pub fn dir(&self, column: usize) -> i32 {
        match self.position(column).map(|i| self.keys[i].1) {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }
    }

    /// Приоритет колонки с единицы; 0, если сортировки нет.
    pub fn priority(&self, column: usize) -> i32 {
        self.position(column)
            .map_or(0, |i| i32::try_from(i + 1).unwrap_or(0))
    }

    /// Щелчок по заголовку: нет → по возрастанию (в конец очереди) → по убыванию → нет.
    pub fn click(&mut self, column: usize) {
        match self.position(column) {
            None => self.keys.push((column, false)),
            Some(i) if !self.keys[i].1 => self.keys[i].1 = true,
            Some(i) => {
                self.keys.remove(i);
            }
        }
    }

    /// Задаёт направление; новая колонка встаёт в конец очереди, у включённой место не меняется.
    pub fn set(&mut self, column: usize, descending: bool) {
        match self.position(column) {
            Some(i) => self.keys[i].1 = descending,
            None => self.keys.push((column, descending)),
        }
    }

    pub fn clear(&mut self, column: usize) {
        if let Some(i) = self.position(column) {
            self.keys.remove(i);
        }
    }

    pub fn clear_all(&mut self) {
        self.keys.clear();
    }

    /// Меняет приоритет: `earlier` ставит колонку выше (раньше), иначе ниже.
    pub fn shift(&mut self, column: usize, earlier: bool) {
        let Some(i) = self.position(column) else {
            return;
        };
        let j = if earlier {
            i.checked_sub(1)
        } else {
            Some(i + 1).filter(|j| *j < self.keys.len())
        };
        if let Some(j) = j {
            self.keys.swap(i, j);
        }
    }

    /// Сравнивает две строки по всем ключам: первый различающийся ключ решает.
    pub fn compare(&self, a: &Receipt, b: &Receipt) -> std::cmp::Ordering {
        self.keys
            .iter()
            .map(|(column, descending)| compare_by(COLUMNS[*column], *descending, a, b))
            .find(|order| order.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Сравнение по одной колонке. Пустой текст всегда в конце, как у ядра, в любом направлении.
fn compare_by(column: Column, descending: bool, a: &Receipt, b: &Receipt) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let order = match column {
        Column::Number => a.number.cmp(&b.number),
        Column::Date => a.date.cmp(&b.date),
        Column::Amount => a.amount.cmp(&b.amount),
        Column::Status => a.posted.cmp(&b.posted),
        Column::Supplier | Column::Warehouse => {
            let (x, y) = if column == Column::Supplier {
                (&a.supplier, &b.supplier)
            } else {
                (&a.warehouse, &b.warehouse)
            };
            match (x.is_empty(), y.is_empty()) {
                (true, true) => return Ordering::Equal,
                (true, false) => return Ordering::Greater,
                (false, true) => return Ordering::Less,
                _ => x.to_lowercase().cmp(&y.to_lowercase()),
            }
        }
    };
    if descending { order.reverse() } else { order }
}

/// Документ, названный в предупреждении ядра: вид («Расход») и номер.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRef {
    pub kind: String,
    pub number: u32,
}

impl DocRef {
    /// Имя объекта в ядре по названию вида документа; неизвестный вид: пусто.
    pub fn object(&self) -> &'static str {
        match self.kind.as_str() {
            "Поступление" => "receipt",
            "Расход" => "issue",
            _ => "",
        }
    }

    /// Подпись ссылки: «Расход №1».
    pub fn title(&self) -> String {
        format!("{} №{}", self.kind, self.number)
    }
}

/// Фраза ядра о документах, которые мешают («Списывают в этот день: Расход №1, Поступление №54.»).
const BLOCKERS: &str = " Списывают в этот день: ";

/// Отделяет документы от текста предупреждения: текст без перечня и список ссылок. Пока ядро
/// отдаёт только строку, ссылки разбираются из неё; когда появятся структурные ссылки в ошибке
/// (запрос к ядру в Вызовы-ядра-из-ostov-ui.md), заменить эту функцию. «Этот документ» не ссылка:
/// он и так открыт. Если ссылок нет, текст остаётся как пришёл.
pub fn split_blockers(message: &str) -> (String, Vec<DocRef>) {
    let Some((text, tail)) = message.split_once(BLOCKERS) else {
        return (message.to_owned(), Vec::new());
    };
    let refs: Vec<DocRef> = tail
        .trim_end_matches('.')
        .split(", ")
        .filter_map(|item| {
            let (kind, number) = item.rsplit_once(" №")?;
            Some(DocRef {
                kind: kind.trim().to_owned(),
                number: number.trim().parse().ok()?,
            })
        })
        .collect();
    if refs.is_empty() {
        (message.to_owned(), Vec::new())
    } else {
        (text.to_owned(), refs)
    }
}

/// Сумма «12 345,60»: неразрывные пробелы между разрядами.
pub fn money(kopecks: i64) -> String {
    let rubles = (kopecks / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in rubles.chars().enumerate() {
        if i > 0 && (rubles.len() - i).is_multiple_of(3) {
            grouped.push('\u{a0}');
        }
        grouped.push(ch);
    }
    format!("{grouped},{:02}", kopecks % 100)
}

pub fn date_text(date: (u16, u8, u8)) -> String {
    format!("{:02}.{:02}.{}", date.2, date.1, date.0)
}

impl Receipt {
    pub fn status(&self) -> &'static str {
        if self.deleted {
            "К удалению"
        } else if self.posted {
            "Проведён"
        } else {
            "Записан"
        }
    }

    /// Значения ячеек в порядке колонок.
    pub fn cells(&self) -> Vec<String> {
        vec![
            self.number.to_string(),
            date_text(self.date),
            self.supplier.clone(),
            self.warehouse.clone(),
            money(self.amount),
            self.status().to_owned(),
        ]
    }
}

/// Разбирает дату «дд.мм.гггг»; неверный текст: `None`.
pub fn parse_date(text: &str) -> Option<(u16, u8, u8)> {
    let mut parts = text.trim().split('.');
    let day: u8 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let year: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month, day))
}

/// Строка табличной части документа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub name: String,
    pub unit: String,
    /// Количество в тысячных долях единицы (ядро хранит три знака).
    pub quantity_milli: i64,
    /// Цена за единицу в копейках.
    pub price: i64,
    /// Сумма строки в копейках.
    pub amount: i64,
}

/// Количество «12,5» или «12,125»: лишние нули в конце не показываем, один знак оставляем.
pub fn quantity(milli: i64) -> String {
    let fraction = format!("{:03}", milli % 1000);
    let fraction = fraction.trim_end_matches('0');
    format!(
        "{},{}",
        milli / 1000,
        if fraction.is_empty() { "0" } else { fraction }
    )
}

impl Line {
    /// Строка из введённых данных: сумма это цена, умноженная на количество.
    pub fn new(name: &str, unit: &str, quantity_milli: i64, price: i64) -> Self {
        Self {
            name: name.to_owned(),
            unit: unit.to_owned(),
            quantity_milli,
            price,
            // Половина вверх, как считает ядро.
            amount: (price * quantity_milli + 500) / 1000,
        }
    }
}

/// Разбирает неотрицательное число с запятой или точкой в целые доли: `scale` знаков после
/// запятой (для количества 1, для денег 2). Лишние знаки отклоняются, а не округляются.
pub fn parse_decimal(text: &str, scale: u32) -> Option<i64> {
    let text = text.trim().replace(',', ".");
    let (whole, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    let scale_len = usize::try_from(scale).ok()?;
    if (whole.is_empty() && fraction.is_empty())
        || !digits(whole)
        || !digits(fraction)
        || fraction.len() > scale_len
    {
        return None;
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let fraction_value: i64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    let missing = u32::try_from(scale_len - fraction.len()).ok()?;
    whole
        .checked_mul(10_i64.pow(scale))?
        .checked_add(fraction_value * 10_i64.pow(missing))
}

/// Оставляет в числе только допустимое: цифры и один разделитель («,» вместо «.»), после
/// запятой не больше `scale` знаков. Нужна, чтобы в поля количества и цены нельзя было
/// ввести буквы и лишние знаки.
pub fn clean_decimal(text: &str, scale: usize) -> String {
    let mut out = String::new();
    let mut separator = false;
    let mut decimals = 0;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !separator {
                out.push(c);
            } else if decimals < scale {
                out.push(c);
                decimals += 1;
            }
        } else if (c == ',' || c == '.') && !separator && scale > 0 {
            separator = true;
            out.push(',');
        }
    }
    out
}

/// «1 позиция», «2 позиции», «5 позиций».
pub fn positions(count: usize) -> String {
    let last = count % 10;
    let last_two = count % 100;
    let word = if last == 1 && last_two != 11 {
        "позиция"
    } else if (2..=4).contains(&last) && !(12..=14).contains(&last_two) {
        "позиции"
    } else {
        "позиций"
    };
    format!("{count} {word}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(number: u32, supplier: &str, amount: i64, posted: bool, day: u8) -> Receipt {
        Receipt {
            id: i64::from(number),
            number,
            date: (2026, 1, day),
            supplier: supplier.to_owned(),
            warehouse: String::new(),
            amount,
            posted,
            deleted: false,
        }
    }

    #[test]
    fn blockers_become_links() {
        let message = "Не хватает остатка на 10.02.2026: Склад «Основной», Товар «Мука» — Количество стало бы −2,000. Списывают в этот день: Расход №1, Поступление №54.";
        let (text, refs) = split_blockers(message);
        assert_eq!(
            text,
            "Не хватает остатка на 10.02.2026: Склад «Основной», Товар «Мука» — Количество стало бы −2,000."
        );
        assert_eq!(
            refs.iter().map(DocRef::title).collect::<Vec<_>>(),
            ["Расход №1", "Поступление №54"]
        );
        assert_eq!(refs[0].object(), "issue");
        assert_eq!(refs[1].object(), "receipt");
    }

    #[test]
    fn this_document_is_not_a_link() {
        let message = "Не хватает остатка на 15.01.2026: Товар «Мука» — Количество стало бы −3,000. Списывают в этот день: этот документ.";
        assert_eq!(split_blockers(message), (message.to_owned(), Vec::new()));
        assert_eq!(
            split_blockers("Другая ошибка."),
            ("Другая ошибка.".to_owned(), Vec::new())
        );
    }

    #[test]
    fn click_cycles_through_three_states() {
        let mut sorting = Sorting::default();
        assert_eq!(sorting.dir(2), 0);
        sorting.click(2);
        assert_eq!(sorting.dir(2), 1);
        sorting.click(2);
        assert_eq!(sorting.dir(2), 2);
        sorting.click(2);
        assert_eq!(sorting.dir(2), 0);
        assert!(sorting.keys().is_empty());
    }

    #[test]
    fn priority_follows_the_order_of_setting() {
        let mut sorting = Sorting::default();
        sorting.click(5);
        sorting.click(2);
        sorting.click(1);
        assert_eq!(
            [5, 2, 1].map(|c| sorting.priority(c)),
            [1, 2, 3],
            "приоритет равен порядку включения"
        );
        // Повторный щелчок по включённой колонке место в очереди не меняет.
        sorting.click(5);
        assert_eq!((sorting.priority(5), sorting.dir(5)), (1, 2));
        sorting.shift(1, true);
        assert_eq!(
            sorting.keys().iter().map(|k| k.0).collect::<Vec<_>>(),
            [5, 1, 2]
        );
        sorting.shift(5, true);
        sorting.shift(2, false);
        assert_eq!(
            sorting.keys().iter().map(|k| k.0).collect::<Vec<_>>(),
            [5, 1, 2]
        );
        sorting.clear(1);
        assert_eq!(sorting.priority(2), 2);
        sorting.set(0, true);
        assert_eq!((sorting.priority(0), sorting.dir(0)), (3, 2));
        sorting.clear_all();
        assert!(sorting.keys().is_empty());
    }

    #[test]
    fn first_key_decides_then_the_next() {
        // Статус, потом поставщик по убыванию, потом номер.
        let mut rows = [
            receipt(1, "Б", 10, true, 1),
            receipt(2, "А", 10, false, 1),
            receipt(3, "Б", 10, false, 1),
            receipt(4, "Б", 10, false, 1),
            receipt(5, "", 10, false, 1),
        ];
        let mut sorting = Sorting::default();
        sorting.set(5, false);
        sorting.set(2, true);
        sorting.set(0, false);
        rows.sort_by(|a, b| sorting.compare(a, b));
        let numbers: Vec<u32> = rows.iter().map(|r| r.number).collect();
        // Не проведённые первыми; среди них «Б» раньше «А», пустой поставщик в конце;
        // одинаковые «Б» по номеру. Проведённый последним.
        assert_eq!(numbers, [3, 4, 2, 5, 1]);
    }

    #[test]
    fn empty_text_stays_last_in_both_directions() {
        let (a, b) = (receipt(1, "", 0, false, 1), receipt(2, "Я", 0, false, 1));
        for descending in [false, true] {
            assert_eq!(
                compare_by(Column::Supplier, descending, &a, &b),
                std::cmp::Ordering::Greater
            );
        }
    }
}
