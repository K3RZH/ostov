//! Список любого объекта по метаданным: какие колонки показать и что стоит в ячейках строки.
//! Чистая логика без интерфейса и без ядра: колонки из `ObjectMeta`, ячейки из `Row`.

use ostov_api::{FieldDef, FieldKind, ObjectKind, ObjectMeta, Row, Value};

use crate::GridColumn;
use crate::values::display;

/// Служебные поля строки документа (их добавляет ядро, в метаданных шапки их нет).
const NUMBER_FIELD: &str = "number";
const POSTED_FIELD: &str = "posted";
const DELETED_FIELD: &str = "deleted";
/// Поле даты документа: в списке стоит сразу после номера, где бы ни было в шапке.
const DATE_FIELD: &str = "date";

const NUMBER_TITLE: &str = "№";
const STATUS_TITLE: &str = "Статус";

// Начальные ширины, px: те же, что были у списка поступлений; дальше их меняет пользователь.
const NUMBER_WIDTH: f32 = 64.0;
const STATUS_WIDTH: f32 = 108.0;
const INT_WIDTH: f32 = 90.0;
const TEXT_WIDTH: f32 = 120.0;
const DECIMAL_WIDTH: f32 = 130.0;
const DATE_WIDTH: f32 = 104.0;

/// Что показывает колонка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnContent {
    /// Номер документа.
    Number,
    /// Статус документа: считается из признаков `posted` и `deleted`.
    Status,
    /// Поле шапки с его типом.
    Field(FieldKind),
}

/// Колонка списка объекта.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSpec {
    pub title: String,
    /// Имя поля в ядре: по нему ядро сортирует, из него же берётся значение ячейки.
    /// У статуса это `posted`.
    pub field: String,
    pub content: ColumnContent,
    /// Начальная ширина, px.
    pub width: f32,
    /// Выравнивание вправо: числа и деньги.
    pub right: bool,
    /// Гибкая колонка забирает свободное место таблицы; в списке она одна или её нет.
    pub flex: bool,
}

impl ColumnSpec {
    fn number() -> Self {
        Self {
            title: NUMBER_TITLE.to_owned(),
            field: NUMBER_FIELD.to_owned(),
            content: ColumnContent::Number,
            width: NUMBER_WIDTH,
            // Номер это обозначение документа, а не величина: читается слева, как текст.
            right: false,
            flex: false,
        }
    }

    fn status() -> Self {
        Self {
            title: STATUS_TITLE.to_owned(),
            field: POSTED_FIELD.to_owned(),
            content: ColumnContent::Status,
            width: STATUS_WIDTH,
            right: false,
            flex: false,
        }
    }

    fn field(def: &FieldDef) -> Self {
        let (width, right) = match def.kind {
            FieldKind::Int => (INT_WIDTH, true),
            FieldKind::Decimal { .. } => (DECIMAL_WIDTH, true),
            FieldKind::Date => (DATE_WIDTH, false),
            FieldKind::Text | FieldKind::Ref => (TEXT_WIDTH, false),
        };
        Self {
            title: def.title.clone(),
            field: def.name.clone(),
            content: ColumnContent::Field(def.kind),
            width,
            right,
            flex: false,
        }
    }

    /// Колонка для таблицы интерфейса (ErpGrid).
    pub fn grid_column(&self) -> GridColumn {
        GridColumn {
            title: self.title.as_str().into(),
            width: self.width,
            right: self.right,
            flex: self.flex,
            status: self.content == ColumnContent::Status,
        }
    }
}

/// Колонки списка объекта по его метаданным. У справочника это поля по порядку. У документа:
/// номер, дата, остальные поля шапки по порядку, в конце статус. Гибкая колонка: первая
/// текстовая или ссылочная.
pub fn columns_from_meta(meta: &ObjectMeta) -> Vec<ColumnSpec> {
    let mut columns: Vec<ColumnSpec> = match meta.kind {
        ObjectKind::Catalog => meta.fields.iter().map(ColumnSpec::field).collect(),
        ObjectKind::Document => {
            let is_date = |def: &&FieldDef| def.name == DATE_FIELD;
            std::iter::once(ColumnSpec::number())
                .chain(meta.fields.iter().filter(is_date).map(ColumnSpec::field))
                .chain(
                    meta.fields
                        .iter()
                        .filter(|def| !is_date(def))
                        .map(ColumnSpec::field),
                )
                .chain(std::iter::once(ColumnSpec::status()))
                .collect()
        }
    };
    let flexible = columns.iter_mut().find(|column| {
        matches!(
            column.content,
            ColumnContent::Field(FieldKind::Text | FieldKind::Ref)
        )
    });
    if let Some(column) = flexible {
        column.flex = true;
    }
    columns
}

/// Ячейки строки списка в порядке колонок. Поля, которого в строке нет, и пустое значение
/// дают пустую ячейку; ссылка показывает название.
pub fn row_cells(row: &Row, columns: &[ColumnSpec]) -> Vec<String> {
    columns
        .iter()
        .map(|column| match column.content {
            ColumnContent::Number => cell(row, &column.field, FieldKind::Int),
            ColumnContent::Status => status(row).to_owned(),
            ColumnContent::Field(kind) => cell(row, &column.field, kind),
        })
        .collect()
}

fn cell(row: &Row, field: &str, kind: FieldKind) -> String {
    row.values
        .get(field)
        .map(|value| display(value, kind))
        .unwrap_or_default()
}

/// Статус документа; пометка на удаление важнее проведения.
fn status(row: &Row) -> &'static str {
    let flag = |name: &str| matches!(row.values.get(name), Some(Value::Bool(true)));
    if flag(DELETED_FIELD) {
        "К удалению"
    } else if flag(POSTED_FIELD) {
        "Проведён"
    } else {
        "Записан"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipts::{COLUMNS, Receipt};
    use crate::receipts_ui::{COLUMN_FIELDS, list_columns};
    use ostov_api::{Meta, Rights};

    fn demo_object(name: &str) -> ObjectMeta {
        let config = ostov_meta::demo().expect("встроенная конфигурация должна читаться");
        Meta::new(&config)
            .objects
            .into_iter()
            .find(|object| object.name == name)
            .expect("объект есть во встроенной конфигурации")
    }

    fn object(kind: ObjectKind, fields: Vec<FieldDef>) -> ObjectMeta {
        ObjectMeta {
            name: "object".to_owned(),
            title: "Объект".to_owned(),
            kind,
            icon: None,
            fields,
            table_parts: Vec::new(),
            rights: Rights::ALL,
        }
    }

    fn row(values: &[(&str, Value)]) -> Row {
        Row {
            id: 1,
            values: values
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        }
    }

    fn reference(id: i64, title: &str) -> Value {
        Value::Ref {
            id,
            title: Some(title.to_owned()),
        }
    }

    #[test]
    fn receipt_columns_match_the_receipts_screen() {
        let columns = columns_from_meta(&demo_object("receipt"));
        let titles: Vec<&str> = columns.iter().map(|c| c.title.as_str()).collect();
        let screen_titles: Vec<&str> = COLUMNS.iter().map(|c| c.title()).collect();
        assert_eq!(titles, screen_titles);
        let fields: Vec<&str> = columns.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(fields, COLUMN_FIELDS);
        // Ширины, выравнивание, гибкая колонка и признак статуса тоже как на экране.
        let grid: Vec<GridColumn> = columns.iter().map(ColumnSpec::grid_column).collect();
        assert_eq!(grid, list_columns());
    }

    #[test]
    fn receipt_cells_match_the_receipts_screen() {
        let columns = columns_from_meta(&demo_object("receipt"));
        let core_row = row(&[
            ("number", Value::Int(54)),
            ("date", Value::Date("2026-10-18".to_owned())),
            ("supplier", reference(3, "Мельница")),
            ("warehouse", reference(1, "Основной")),
            (
                "amount",
                Value::Decimal {
                    units: 1_234_560,
                    scale: 2,
                },
            ),
            ("posted", Value::Bool(true)),
            ("deleted", Value::Bool(false)),
            ("version", Value::Int(2)),
        ]);
        let receipt = Receipt {
            id: 1,
            number: 54,
            date: (2026, 10, 18),
            supplier: "Мельница".to_owned(),
            warehouse: "Основной".to_owned(),
            amount: 1_234_560,
            posted: true,
            deleted: false,
        };
        assert_eq!(row_cells(&core_row, &columns), receipt.cells());
    }

    #[test]
    fn document_puts_number_and_date_first_and_status_last() {
        let meta = object(
            ObjectKind::Document,
            vec![
                FieldDef::new("comment", FieldKind::Text).with_title("Комментарий"),
                FieldDef::new("date", FieldKind::Date).with_title("Дата"),
                FieldDef::new("count", FieldKind::Int).with_title("Мест"),
            ],
        );
        let columns = columns_from_meta(&meta);
        let fields: Vec<&str> = columns.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(fields, ["number", "date", "comment", "count", "posted"]);
        assert_eq!(columns[0].content, ColumnContent::Number);
        assert_eq!(columns[4].content, ColumnContent::Status);
        assert!(columns[4].grid_column().status);
    }

    #[test]
    fn catalog_columns_are_its_fields_in_order() {
        let columns = columns_from_meta(&demo_object("goods"));
        let titles: Vec<&str> = columns.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Наименование", "Единица измерения"]);
        let fields: Vec<&str> = columns.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(fields, ["name", "unit"]);
        // Гибкая только первая текстовая; номера и статуса у справочника нет.
        let flex: Vec<bool> = columns.iter().map(|c| c.flex).collect();
        assert_eq!(flex, [true, false]);
        let cells = row_cells(
            &row(&[
                ("name", Value::Text("Мука".to_owned())),
                ("unit", Value::Text("кг".to_owned())),
                ("deleted", Value::Bool(false)),
            ]),
            &columns,
        );
        assert_eq!(cells, ["Мука", "кг"]);
    }

    #[test]
    fn width_and_alignment_follow_the_field_kind() {
        let meta = object(
            ObjectKind::Catalog,
            vec![
                FieldDef::new("count", FieldKind::Int),
                FieldDef::new("price", FieldKind::Decimal { scale: 2 }),
                FieldDef::new("since", FieldKind::Date),
                FieldDef::reference("supplier", "suppliers"),
                FieldDef::new("name", FieldKind::Text),
            ],
        );
        let columns = columns_from_meta(&meta);
        let right: Vec<bool> = columns.iter().map(|c| c.right).collect();
        assert_eq!(right, [true, true, false, false, false]);
        let widths: Vec<f32> = columns.iter().map(|c| c.width).collect();
        assert_eq!(
            widths,
            [INT_WIDTH, DECIMAL_WIDTH, DATE_WIDTH, TEXT_WIDTH, TEXT_WIDTH]
        );
        // Первая текстовая или ссылочная: здесь это ссылка, текст после неё уже не гибкий.
        let flex: Vec<bool> = columns.iter().map(|c| c.flex).collect();
        assert_eq!(flex, [false, false, false, true, false]);
    }

    #[test]
    fn list_without_text_columns_has_no_flex_column() {
        let meta = object(
            ObjectKind::Catalog,
            vec![FieldDef::new("count", FieldKind::Int)],
        );
        assert!(columns_from_meta(&meta).iter().all(|c| !c.flex));
    }

    #[test]
    fn empty_values_give_empty_cells() {
        let columns = columns_from_meta(&demo_object("receipt"));
        // Поля нет в строке совсем.
        assert_eq!(
            row_cells(&row(&[]), &columns),
            ["", "", "", "", "", "Записан"]
        );
        // Поле есть, но пустое.
        let nulls = row(&[
            ("number", Value::Null),
            ("date", Value::Null),
            ("supplier", Value::Null),
            ("warehouse", Value::Null),
            ("amount", Value::Null),
        ]);
        assert_eq!(row_cells(&nulls, &columns), ["", "", "", "", "", "Записан"]);
    }

    #[test]
    fn reference_without_title_shows_its_id() {
        let columns = columns_from_meta(&demo_object("receipt"));
        let cells = row_cells(
            &row(&[("supplier", Value::Ref { id: 7, title: None })]),
            &columns,
        );
        assert_eq!(cells[2], "7");
    }

    #[test]
    fn decimal_is_shown_with_the_scale_of_the_field() {
        let meta = object(
            ObjectKind::Catalog,
            vec![
                FieldDef::new("price", FieldKind::Decimal { scale: 2 }),
                FieldDef::new("qty", FieldKind::Decimal { scale: 3 }),
            ],
        );
        let columns = columns_from_meta(&meta);
        let decimal = |units, scale| Value::Decimal { units, scale };
        let cells = row_cells(
            &row(&[
                ("price", decimal(1_234_560, 2)),
                ("qty", decimal(12_500, 3)),
            ]),
            &columns,
        );
        assert_eq!(cells, ["12\u{a0}345,60", "12,500"]);
        // Знаки задаёт поле: лишние у значения отбрасываются, недостающие дополняются нулями.
        let cells = row_cells(
            &row(&[("price", decimal(12_345, 3)), ("qty", decimal(-5, 0))]),
            &columns,
        );
        assert_eq!(cells, ["12,34", "-5,000"]);
    }

    #[test]
    fn status_comes_from_posted_and_deleted() {
        let columns = columns_from_meta(&demo_object("receipt"));
        let status_of = |posted: bool, deleted: bool| {
            let cells = row_cells(
                &row(&[
                    ("posted", Value::Bool(posted)),
                    ("deleted", Value::Bool(deleted)),
                ]),
                &columns,
            );
            cells.last().cloned().unwrap_or_default()
        };
        assert_eq!(status_of(false, false), "Записан");
        assert_eq!(status_of(true, false), "Проведён");
        // Пометка на удаление важнее проведения.
        assert_eq!(status_of(true, true), "К удалению");
        assert_eq!(status_of(false, true), "К удалению");
    }
}
