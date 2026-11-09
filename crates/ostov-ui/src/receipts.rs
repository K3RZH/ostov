//! Данные списка «Поступления» и строки документа: типы интерфейса. Сами данные, порядок и поиск
//! дают ядро (Engine API); показ и разбор значений в `values`, ключи сортировки в `sorting`.

use crate::sorting::Sorting;
use crate::values::{date_text, money};

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

/// Сравнивает две строки по ключам сортировки: первый различающийся ключ решает. Нужна для
/// досортировки здесь, когда ключей несколько: ядро сортирует только по одному полю.
pub fn compare(sorting: &Sorting, a: &Receipt, b: &Receipt) -> std::cmp::Ordering {
    sorting
        .keys()
        .iter()
        .map(|(column, descending)| compare_by(COLUMNS[*column], *descending, a, b))
        .find(|order| order.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
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
        rows.sort_by(|a, b| compare(&sorting, a, b));
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
