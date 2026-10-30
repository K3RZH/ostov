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
