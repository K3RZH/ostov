//! Данные списка «Поступления» для макета: примерные документы, сортировка, поиск.
//! Настоящие данные придут из ядра (Engine API); логика сортировки и отбора останется.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub number: u32,
    /// Дата как (год, месяц, день): так сортировка совпадает с календарной.
    pub date: (u16, u8, u8),
    pub supplier: String,
    pub warehouse: String,
    /// Сумма в копейках: деньги не храним дробным числом.
    pub amount: i64,
    pub posted: bool,
    /// Строки документа; `None`: ещё не открывали (покажем примерные).
    pub lines: Option<Vec<Line>>,
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

/// Справочник поставщиков (для выпадающего списка).
pub const SUPPLIERS: [&str; 6] = [
    "Мясной двор",
    "Овощи оптом",
    "Молочный комбинат",
    "Бакалея-Центр",
    "Рыбный ряд",
    "Пекарня №3",
];
/// Перечисление складов (для выпадающего списка).
pub const WAREHOUSES: [&str; 3] = ["Основной", "Кухня", "Бар"];

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

/// Примерные документы: детерминированный генератор, чтобы список не менялся между запусками.
pub fn demo(count: u32) -> Vec<Receipt> {
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    (0..count)
        .map(|i| {
            let r = next();
            Receipt {
                number: count - i,
                date: (
                    2026,
                    1 + u8::try_from((r >> 8) % 10).unwrap_or(0),
                    1 + u8::try_from((r >> 16) % 28).unwrap_or(0),
                ),
                supplier: SUPPLIERS[usize::try_from((r >> 24) % 6).unwrap_or(0)].to_owned(),
                warehouse: WAREHOUSES[usize::try_from((r >> 32) % 3).unwrap_or(0)].to_owned(),
                amount: i64::try_from((r >> 40) % 9_000_000).unwrap_or(0) + 10_000,
                posted: (r >> 60) % 4 != 0,
                lines: None,
            }
        })
        .collect()
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
        if self.posted {
            "Проведён"
        } else {
            "Черновик"
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

    fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self
                .cells()
                .iter()
                .any(|cell| cell.to_lowercase().contains(&query))
    }
}

/// Сортирует по колонке; при равенстве порядок сохраняется (сортировка устойчивая).
pub fn sort(receipts: &mut [Receipt], column: Column, descending: bool) {
    receipts.sort_by(|a, b| {
        let order = match column {
            Column::Number => a.number.cmp(&b.number),
            Column::Date => a.date.cmp(&b.date),
            Column::Supplier => a.supplier.cmp(&b.supplier),
            Column::Warehouse => a.warehouse.cmp(&b.warehouse),
            Column::Amount => a.amount.cmp(&b.amount),
            Column::Status => a.posted.cmp(&b.posted),
        };
        if descending { order.reverse() } else { order }
    });
}

/// Документы, подходящие под строку поиска (по любой ячейке, без учёта регистра).
pub fn filter<'a>(receipts: &'a [Receipt], query: &str) -> Vec<&'a Receipt> {
    receipts.iter().filter(|r| r.matches(query)).collect()
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
    /// Количество в десятых долях единицы.
    pub quantity_tenths: i64,
    /// Цена за единицу в копейках.
    pub price: i64,
    /// Сумма строки в копейках.
    pub amount: i64,
}

/// Справочник номенклатуры: название и единица измерения.
pub const GOODS: [(&str, &str); 8] = [
    ("Мука пшеничная в/с", "кг"),
    ("Говядина охлаждённая", "кг"),
    ("Молоко 3,2%", "л"),
    ("Сахар-песок", "кг"),
    ("Масло сливочное 82%", "кг"),
    ("Яйцо куриное С1", "шт"),
    ("Картофель", "кг"),
    ("Томаты", "кг"),
];

/// Примерные строки документа: детерминированные, сумма строк равна сумме документа.
pub fn lines(receipt: &Receipt) -> Vec<Line> {
    let count = 3 + usize::try_from(receipt.number % 4).unwrap_or(0);
    let weights: Vec<i64> = (0..count)
        .map(|i| {
            1 + i64::from(
                receipt
                    .number
                    .wrapping_mul(31)
                    .wrapping_add(u32::try_from(i).unwrap_or(0) * 17)
                    % 7,
            )
        })
        .collect();
    let total_weight: i64 = weights.iter().sum();
    let mut left = receipt.amount;
    weights
        .iter()
        .enumerate()
        .map(|(i, weight)| {
            let amount = if i + 1 == count {
                left
            } else {
                receipt.amount * weight / total_weight
            };
            left -= amount;
            let (name, unit) =
                GOODS[(usize::try_from(receipt.number).unwrap_or(0) + i * 3) % GOODS.len()];
            let quantity_tenths =
                100 + i64::from(receipt.number % 9) * 70 + i64::try_from(i).unwrap_or(0) * 35;
            Line {
                name: name.to_owned(),
                unit: unit.to_owned(),
                quantity_tenths,
                price: amount * 10 / quantity_tenths.max(1),
                amount,
            }
        })
        .collect()
}

/// Количество «12,5».
pub fn quantity(tenths: i64) -> String {
    format!("{},{}", tenths / 10, tenths % 10)
}

impl Line {
    /// Строка из введённых данных: сумма это цена, умноженная на количество.
    pub fn new(name: &str, quantity_tenths: i64, price: i64) -> Self {
        Self {
            name: name.to_owned(),
            unit: unit_of(name).to_owned(),
            quantity_tenths,
            price,
            amount: price * quantity_tenths / 10,
        }
    }
}

/// Единица измерения товара по справочнику (нет в справочнике: «шт»).
pub fn unit_of(name: &str) -> &'static str {
    GOODS
        .iter()
        .find(|(good, _)| *good == name)
        .map_or("шт", |(_, unit)| unit)
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
