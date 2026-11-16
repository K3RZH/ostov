//! Табличные части формы документа: строки, окно строки и проверка обязательных колонок.
//!
//! Чистая логика без интерфейса и без ядра. Строка хранится значениями (`Fields`), ссылки вместе с
//! названиями: таблица показывает названия, а в ядро ссылка уходит одним `id`. Строку правят в
//! окне строки (`RowDraft`): ячейки разбираются так же, как поля шапки, и попадают в таблицу по
//! «Готово», только если обязательные заполнены. Перед записью документа проверяются все строки:
//! у строки без обязательного значения остаётся ошибка с названиями пустых колонок. Табличные
//! части, которых нет в метаданных, хранятся как пришли и уходят назад без изменений.

use std::collections::BTreeMap;
use std::fmt;

use ostov_api::{FieldDef, Fields, TablePartDef, Value};

use crate::form_state::{FormField, core_value};
use crate::values::display;

/// Одна табличная часть: колонки из метаданных, строки и выбранная строка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePart {
    pub name: String,
    pub title: String,
    columns: Vec<FieldDef>,
    rows: Vec<Fields>,
    /// Строки с открытия или последней записи: от них считается «изменено».
    original: Vec<Fields>,
    /// Ошибка строки после проверки перед записью; `None`: строка заполнена.
    errors: Vec<Option<String>>,
    selected: Option<usize>,
}

impl TablePart {
    fn new(def: &TablePartDef, rows: Vec<Fields>) -> Self {
        Self {
            name: def.name.clone(),
            title: def.title.clone(),
            columns: def.fields.clone(),
            errors: vec![None; rows.len()],
            original: rows.clone(),
            rows,
            selected: None,
        }
    }

    pub fn columns(&self) -> &[FieldDef] {
        &self.columns
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Ячейка строки `row` в колонке `column` текстом; нет значения: пусто.
    pub fn cell_text(&self, row: usize, column: usize) -> String {
        let (Some(values), Some(def)) = (self.rows.get(row), self.columns.get(column)) else {
            return String::new();
        };
        values
            .get(&def.name)
            .map(|value| display(value, def.kind))
            .unwrap_or_default()
    }

    /// Ошибка строки, найденная проверкой перед записью.
    pub fn row_error(&self, row: usize) -> Option<&str> {
        self.errors.get(row).and_then(Option::as_deref)
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    fn is_changed(&self) -> bool {
        self.rows.len() != self.original.len()
            || self
                .rows
                .iter()
                .zip(&self.original)
                .any(|(row, original)| !same_row(row, original))
    }

    /// Отмечает строки без обязательных значений; возвращает, сколько их.
    fn check(&mut self) -> usize {
        let errors: Vec<Option<String>> = self
            .rows
            .iter()
            .map(|row| missing_text(&self.columns, row))
            .collect();
        self.errors = errors;
        self.errors.iter().filter(|error| error.is_some()).count()
    }

    fn to_core(&self) -> Vec<Fields> {
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(name, value)| (name.clone(), core_value(value)))
                    .collect()
            })
            .collect()
    }

    fn remove(&mut self, row: usize) -> bool {
        if row >= self.rows.len() {
            return false;
        }
        self.rows.remove(row);
        if row < self.errors.len() {
            self.errors.remove(row);
        }
        self.selected = match self.selected {
            Some(selected) if selected > row => Some(selected - 1),
            // Выбор встаёт на соседнюю строку: удалять подряд можно, не трогая мышь.
            Some(selected) if selected == row => {
                (!self.rows.is_empty()).then(|| row.min(self.rows.len() - 1))
            }
            other => other,
        };
        true
    }
}

/// Одинаковы ли строки по значениям для ядра: название ссылки и отсутствие ключа против пустого
/// значения разницей не считаются.
fn same_row(row: &Fields, other: &Fields) -> bool {
    let value = |fields: &Fields, name: &str| fields.get(name).map_or(Value::Null, core_value);
    row.keys()
        .chain(other.keys())
        .all(|name| value(row, name) == value(other, name))
}

/// «Заполните: Товар, Количество.» по пустым обязательным колонкам строки; `None`: всё заполнено.
fn missing_text(columns: &[FieldDef], row: &Fields) -> Option<String> {
    let missing: Vec<&str> = columns
        .iter()
        .filter(|def| def.required && !def.read_only)
        .filter(|def| matches!(row.get(&def.name), None | Some(Value::Null)))
        .map(|def| def.title.as_str())
        .collect();
    (!missing.is_empty()).then(|| format!("Заполните: {}.", missing.join(", ")))
}

/// «Готово» не прошло: в строке есть ячейки с ошибкой (они отмечены).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidRow {
    pub cells: usize,
}

impl fmt::Display for InvalidRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Строка не готова: исправьте отмеченные поля ({}).",
            self.cells
        )
    }
}

impl std::error::Error for InvalidRow {}

/// Окно строки: ячейки одной строки, пока их правят. В таблицу они попадают по «Готово».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowDraft {
    part: usize,
    /// Номер правимой строки; `None`: новая строка, по «Готово» встанет в конец.
    row: Option<usize>,
    cells: Vec<FormField>,
    editable: bool,
    error: Option<String>,
}

impl RowDraft {
    fn new(part: &TablePart, index: usize, row: Option<usize>, editable: bool) -> Self {
        let values = row.and_then(|row| part.rows.get(row));
        let cells = part
            .columns
            .iter()
            .map(|def| {
                let value = values
                    .and_then(|values| values.get(&def.name))
                    .cloned()
                    .unwrap_or(Value::Null);
                let mut cell = FormField::new(def, value);
                cell.read_only |= !editable;
                cell
            })
            .collect();
        Self {
            part: index,
            row,
            cells,
            editable,
            error: None,
        }
    }

    pub fn part(&self) -> usize {
        self.part
    }

    /// Номер строки в таблице; `None`: строка новая.
    pub fn row(&self) -> Option<usize> {
        self.row
    }

    pub fn cells(&self) -> &[FormField] {
        &self.cells
    }

    /// Строку можно править; иначе окно только показывает её.
    pub fn is_editable(&self) -> bool {
        self.editable
    }

    /// Ячейку `cell` можно править текстом.
    pub fn accepts_text(&self, cell: usize) -> bool {
        self.cells.get(cell).is_some_and(FormField::accepts_text)
    }

    /// В ячейку `cell` значение выбирают из справочника.
    pub fn accepts_choice(&self, cell: usize) -> bool {
        self.cells.get(cell).is_some_and(FormField::accepts_choice)
    }

    /// Почему «Готово» не прошло.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn is_changed(&self) -> bool {
        self.cells.iter().any(FormField::is_changed)
    }
}

/// Табличные части формы и открытое окно строки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parts {
    parts: Vec<TablePart>,
    /// Табличные части из ядра, которых нет в метаданных: уходят назад как пришли.
    other: BTreeMap<String, Vec<Fields>>,
    /// В коробке: форма лежит в перечислении рядом с пустыми вариантами и не должна расти.
    draft: Option<Box<RowDraft>>,
}

impl Parts {
    /// Табличные части по метаданным; строки берутся из ответа ядра, если он есть.
    pub fn new(defs: &[TablePartDef], tables: Option<&BTreeMap<String, Vec<Fields>>>) -> Self {
        let rows = |name: &str| {
            tables
                .and_then(|tables| tables.get(name))
                .cloned()
                .unwrap_or_default()
        };
        let other = tables
            .into_iter()
            .flatten()
            .filter(|(name, _)| !defs.iter().any(|def| &def.name == *name))
            .map(|(name, rows)| (name.clone(), rows.clone()))
            .collect();
        Self {
            parts: defs
                .iter()
                .map(|def| TablePart::new(def, rows(&def.name)))
                .collect(),
            other,
            draft: None,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &TablePart> {
        self.parts.iter()
    }

    pub fn get(&self, part: usize) -> Option<&TablePart> {
        self.parts.get(part)
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Открытое окно строки.
    pub fn draft(&self) -> Option<&RowDraft> {
        self.draft.as_deref()
    }

    pub(crate) fn select(&mut self, part: usize, row: usize) {
        if let Some(part) = self.parts.get_mut(part).filter(|p| row < p.rows.len()) {
            part.selected = Some(row);
        }
    }

    pub(crate) fn open_new(&mut self, part: usize) -> bool {
        self.open(part, None, true)
    }

    pub(crate) fn open_row(&mut self, part: usize, row: usize, editable: bool) -> bool {
        let exists = self.parts.get(part).is_some_and(|p| row < p.rows.len());
        exists && self.open(part, Some(row), editable)
    }

    fn open(&mut self, index: usize, row: Option<usize>, editable: bool) -> bool {
        if self.draft.is_some() {
            return false;
        }
        let Some(part) = self.parts.get_mut(index) else {
            return false;
        };
        if let Some(row) = row {
            part.selected = Some(row);
        }
        self.draft = Some(Box::new(RowDraft::new(part, index, row, editable)));
        true
    }

    pub(crate) fn edit_cell(&mut self, cell: usize, text: &str) {
        if let Some(cell) = self.draft_cell(cell).filter(|c| c.accepts_text()) {
            cell.apply_text(text);
        }
    }

    pub(crate) fn choose_cell(&mut self, cell: usize, chosen: Option<(i64, String)>) {
        if let Some(cell) = self.draft_cell(cell).filter(|c| c.accepts_choice()) {
            cell.choose(chosen);
        }
    }

    fn draft_cell(&mut self, cell: usize) -> Option<&mut FormField> {
        self.draft.as_mut()?.cells.get_mut(cell)
    }

    pub(crate) fn commit(&mut self) -> Result<(), InvalidRow> {
        let Some(draft) = self.draft.as_mut() else {
            return Ok(());
        };
        if !draft.editable {
            self.draft = None;
            return Ok(());
        }
        let invalid = draft
            .cells
            .iter_mut()
            .map(FormField::check_required)
            .filter(|invalid| *invalid)
            .count();
        if invalid > 0 {
            let error = InvalidRow { cells: invalid };
            draft.error = Some(error.to_string());
            return Err(error);
        }
        let Some(draft) = self.draft.take() else {
            return Ok(());
        };
        let Some(part) = self.parts.get_mut(draft.part) else {
            return Ok(());
        };
        // Ключи строки, которых нет в колонках, остаются как пришли из ядра.
        let mut values = draft
            .row
            .and_then(|row| part.rows.get(row))
            .cloned()
            .unwrap_or_default();
        for cell in &draft.cells {
            values.insert(cell.name.clone(), cell.value().clone());
        }
        match draft.row {
            Some(row) => {
                if let Some(slot) = part.rows.get_mut(row) {
                    *slot = values;
                }
                if let Some(error) = part.errors.get_mut(row) {
                    *error = None;
                }
                part.selected = Some(row);
            }
            None => {
                part.rows.push(values);
                part.errors.push(None);
                part.selected = Some(part.rows.len() - 1);
            }
        }
        Ok(())
    }

    pub(crate) fn cancel(&mut self) {
        self.draft = None;
    }

    pub(crate) fn remove(&mut self, part: usize, row: usize) -> bool {
        self.draft.is_none() && self.parts.get_mut(part).is_some_and(|p| p.remove(row))
    }

    pub(crate) fn is_changed(&self) -> bool {
        self.parts.iter().any(TablePart::is_changed)
            || self.draft.as_deref().is_some_and(RowDraft::is_changed)
    }

    /// Проверяет строки всех частей; возвращает, сколько строк с ошибкой.
    pub(crate) fn check(&mut self) -> usize {
        self.parts.iter_mut().map(TablePart::check).sum()
    }

    /// Строки для ядра: ссылки без названий, части не из метаданных как пришли.
    pub(crate) fn to_core(&self) -> BTreeMap<String, Vec<Fields>> {
        let mut tables = self.other.clone();
        for part in &self.parts {
            tables.insert(part.name.clone(), part.to_core());
        }
        tables
    }

    /// Ядро записало документ: текущие строки становятся записанными.
    pub(crate) fn mark_saved(&mut self) {
        for part in &mut self.parts {
            part.original.clone_from(&part.rows);
            part.errors = vec![None; part.rows.len()];
        }
    }
}

#[cfg(test)]
mod tests {
    use ostov_api::FieldKind;

    use super::*;

    fn lines_def() -> TablePartDef {
        let config = ostov_meta::demo().unwrap_or_else(|errors| unreachable!("{errors:?}"));
        config
            .documents
            .iter()
            .find(|document| document.name == "issue")
            .and_then(|document| document.table_parts.first().cloned())
            .unwrap_or_else(|| unreachable!("у расхода нет табличной части"))
    }

    fn reference(id: i64, title: &str) -> Value {
        Value::Ref {
            id,
            title: Some(title.to_owned()),
        }
    }

    fn qty(units: i64) -> Value {
        Value::Decimal { units, scale: 3 }
    }

    fn line(goods: i64, units: i64) -> Fields {
        [
            ("goods".to_owned(), reference(goods, "Мука")),
            ("qty".to_owned(), qty(units)),
        ]
        .into()
    }

    fn parts(rows: Vec<Fields>) -> Parts {
        let tables: BTreeMap<String, Vec<Fields>> = [("lines".to_owned(), rows)].into();
        Parts::new(&[lines_def()], Some(&tables))
    }

    fn part(parts: &Parts) -> &TablePart {
        parts.get(0).unwrap_or_else(|| unreachable!("нет части"))
    }

    #[test]
    fn columns_and_cells_come_from_meta_and_rows() {
        let parts = parts(vec![line(5, 2500)]);
        let lines = part(&parts);
        assert_eq!(lines.title, "Товары");
        let titles: Vec<&str> = lines.columns().iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Товар", "Количество"]);
        assert_eq!(lines.cell_text(0, 0), "Мука");
        assert_eq!(lines.cell_text(0, 1), "2,500");
        assert_eq!(lines.cell_text(3, 0), "");
        assert!(!parts.is_changed());
    }

    #[test]
    fn new_row_goes_to_the_end_only_when_required_cells_are_filled() {
        let mut parts = parts(vec![line(5, 2500)]);
        assert!(parts.open_new(0));
        // Пока окно строки открыто, второе не открывается и строки не удаляются.
        assert!(!parts.open_new(0));
        assert!(!parts.remove(0, 0));
        let invalid = parts.commit().unwrap_err();
        assert_eq!(invalid, InvalidRow { cells: 2 });
        let draft = parts.draft().unwrap();
        assert_eq!(draft.cells()[0].error(), Some("Выберите значение."));
        assert_eq!(draft.cells()[1].error(), Some("Заполните поле."));
        assert!(draft.error().is_some_and(|e| e.contains("(2)")));

        parts.choose_cell(0, Some((7, "Сахар".to_owned())));
        parts.edit_cell(1, "1,5");
        assert!(parts.is_changed());
        assert!(parts.commit().is_ok());
        assert!(parts.draft().is_none());
        let lines = part(&parts);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.cell_text(1, 0), "Сахар");
        assert_eq!(lines.cell_text(1, 1), "1,500");
        assert_eq!(lines.selected(), Some(1));
        assert!(parts.is_changed());
    }

    #[test]
    fn edited_row_replaces_the_old_one_and_cancel_keeps_it() {
        let mut parts = parts(vec![line(5, 2500)]);
        assert!(parts.open_row(0, 0, true));
        parts.edit_cell(1, "3");
        parts.cancel();
        assert_eq!(part(&parts).cell_text(0, 1), "2,500");
        assert!(!parts.is_changed());

        assert!(parts.open_row(0, 0, true));
        // Ссылку текстом не набрать, число не выбрать из справочника.
        parts.edit_cell(0, "Сахар");
        parts.choose_cell(1, Some((1, "x".to_owned())));
        parts.edit_cell(1, "3");
        parts.commit().unwrap();
        assert_eq!(part(&parts).cell_text(0, 0), "Мука");
        assert_eq!(part(&parts).cell_text(0, 1), "3,000");
        assert!(parts.is_changed());
    }

    #[test]
    fn parse_error_in_a_cell_keeps_the_window_open() {
        let mut parts = parts(Vec::new());
        parts.open_new(0);
        parts.choose_cell(0, Some((5, "Мука".to_owned())));
        parts.edit_cell(1, "1,2345");
        assert_eq!(parts.commit(), Err(InvalidRow { cells: 1 }));
        assert!(parts.draft().is_some());
        assert!(part(&parts).is_empty());
    }

    #[test]
    fn rows_without_required_values_are_marked_before_saving() {
        let mut parts = parts(vec![
            line(5, 1000),
            [("qty".to_owned(), qty(500))].into(),
            Fields::new(),
        ]);
        assert_eq!(parts.check(), 2);
        let lines = part(&parts);
        assert_eq!(lines.row_error(0), None);
        assert_eq!(lines.row_error(1), Some("Заполните: Товар."));
        assert_eq!(lines.row_error(2), Some("Заполните: Товар, Количество."));
        // Исправленная строка ошибку теряет.
        parts.open_row(0, 1, true);
        parts.choose_cell(0, Some((5, "Мука".to_owned())));
        parts.commit().unwrap();
        assert_eq!(part(&parts).row_error(1), None);
        assert_eq!(
            part(&parts).row_error(2),
            Some("Заполните: Товар, Количество.")
        );
    }

    #[test]
    fn removing_rows_moves_the_selection_and_errors() {
        let mut parts = parts(vec![line(1, 1000), Fields::new(), line(3, 3000)]);
        parts.check();
        parts.select(0, 1);
        assert!(parts.remove(0, 0));
        let lines = part(&parts);
        assert_eq!(lines.selected(), Some(0));
        assert!(lines.row_error(0).is_some());
        assert!(parts.remove(0, 0));
        assert_eq!(part(&parts).selected(), Some(0));
        assert_eq!(part(&parts).row_error(0), None);
        assert!(parts.remove(0, 0));
        assert_eq!(part(&parts).selected(), None);
        assert!(!parts.remove(0, 0));
        assert!(!parts.remove(5, 0));
    }

    #[test]
    fn core_gets_ids_without_titles_and_unknown_parts_unchanged() {
        let mut tables: BTreeMap<String, Vec<Fields>> =
            [("lines".to_owned(), vec![line(5, 2500)])].into();
        let unknown = vec![[("note".to_owned(), Value::Text("x".to_owned()))].into()];
        tables.insert("services".to_owned(), unknown.clone());
        let parts = Parts::new(&[lines_def()], Some(&tables));
        let core = parts.to_core();
        assert_eq!(core["services"], unknown);
        assert_eq!(core["lines"][0]["goods"], Value::Ref { id: 5, title: None });
        assert_eq!(core["lines"][0]["qty"], qty(2500));
    }

    #[test]
    fn keys_outside_the_columns_survive_editing() {
        let mut row = line(5, 2500);
        row.insert("line_no".to_owned(), Value::Int(1));
        let mut parts = parts(vec![row]);
        parts.open_row(0, 0, true);
        parts.edit_cell(1, "1");
        parts.commit().unwrap();
        assert_eq!(parts.to_core()["lines"][0]["line_no"], Value::Int(1));
    }

    #[test]
    fn title_or_missing_key_is_not_a_change() {
        let with_title = line(5, 2500);
        let mut without_title = with_title.clone();
        without_title.insert("goods".to_owned(), Value::Ref { id: 5, title: None });
        without_title.insert("price".to_owned(), Value::Null);
        assert!(same_row(&with_title, &without_title));
        without_title.insert("qty".to_owned(), qty(1));
        assert!(!same_row(&with_title, &without_title));
    }

    #[test]
    fn read_only_row_window_only_shows_the_row() {
        let mut parts = parts(vec![line(5, 2500)]);
        assert!(parts.open_row(0, 0, false));
        let draft = parts.draft().unwrap();
        assert!(!draft.is_editable());
        assert!(!draft.accepts_text(1) && !draft.accepts_choice(0));
        parts.edit_cell(1, "9");
        assert!(parts.commit().is_ok());
        assert!(parts.draft().is_none() && !parts.is_changed());
    }

    #[test]
    fn saved_rows_become_the_original() {
        let mut parts = parts(Vec::new());
        parts.open_new(0);
        parts.choose_cell(0, Some((5, "Мука".to_owned())));
        parts.edit_cell(1, "1");
        parts.commit().unwrap();
        assert!(parts.is_changed());
        parts.mark_saved();
        assert!(!parts.is_changed());
    }

    #[test]
    fn computed_column_is_not_edited_in_the_row_window() {
        let mut def = lines_def();
        def.fields.push(FieldDef {
            read_only: true,
            ..FieldDef::new("amount", FieldKind::Decimal { scale: 2 })
        });
        let mut parts = Parts::new(&[def], None);
        parts.open_new(0);
        parts.edit_cell(2, "100");
        assert!(!parts.draft().unwrap().accepts_text(2));
        assert_eq!(*parts.draft().unwrap().cells()[2].value(), Value::Null);
    }
}
