use std::collections::BTreeMap;

use ostov_store::{Driver, Value};

use super::{
    FieldDef, Fields, check_fields, check_ident, column_defs, names, placeholders, read_fields,
    returned_id, to_values,
};
use crate::CoreError;

/// Табличная часть документа, например «Товары» в поступлении.
#[derive(Debug, Clone)]
pub struct TablePart {
    name: String,
    fields: Vec<FieldDef>,
}

impl TablePart {
    pub fn new(name: &str, fields: Vec<FieldDef>) -> Result<Self, CoreError> {
        check_ident(name)?;
        check_fields(&fields, &["id", "doc_id", "line_no"])?;
        Ok(Self {
            name: name.to_owned(),
            fields,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub id: i64,
    pub version: i64,
    pub posted: bool,
    pub header: Fields,
    pub parts: BTreeMap<String, Vec<Fields>>,
}

/// Вид документа: шапка и табличные части. Строки частей лежат в таблицах `<документ>__<часть>`.
#[derive(Debug, Clone)]
pub struct DocumentKind {
    table: String,
    header: Vec<FieldDef>,
    parts: Vec<TablePart>,
}

impl DocumentKind {
    pub fn new(
        table: &str,
        header: Vec<FieldDef>,
        parts: Vec<TablePart>,
    ) -> Result<Self, CoreError> {
        check_ident(table)?;
        check_fields(&header, &["id", "version", "posted"])?;
        for part in &parts {
            check_ident(&format!("{table}__{}", part.name))?;
        }
        Ok(Self {
            table: table.to_owned(),
            header,
            parts,
        })
    }

    pub fn table(&self) -> &str {
        &self.table
    }

    pub fn create_tables(&self, db: &mut dyn Driver) -> Result<(), CoreError> {
        let mut statements = Vec::new();
        {
            let d = db.dialect();
            let mut columns = vec![
                format!("{} {}", d.quote_ident("id"), d.id_column()),
                format!("{} BIGINT NOT NULL", d.quote_ident("version")),
                format!("{} BIGINT NOT NULL", d.quote_ident("posted")),
            ];
            columns.extend(column_defs(d, &self.header));
            statements.push(format!(
                "CREATE TABLE {} ({})",
                d.quote_ident(&self.table),
                columns.join(", ")
            ));

            for part in &self.parts {
                let mut columns = vec![
                    format!("{} {}", d.quote_ident("id"), d.id_column()),
                    format!(
                        "{} BIGINT NOT NULL REFERENCES {}",
                        d.quote_ident("doc_id"),
                        d.quote_ident(&self.table)
                    ),
                    format!("{} BIGINT NOT NULL", d.quote_ident("line_no")),
                ];
                columns.extend(column_defs(d, &part.fields));
                statements.push(format!(
                    "CREATE TABLE {} ({})",
                    d.quote_ident(&self.part_table(part)),
                    columns.join(", ")
                ));
            }
        }
        for sql in statements {
            db.execute(&sql, &[])?;
        }
        Ok(())
    }

    /// Записывает новый документ, ещё не проведённый.
    pub fn insert(
        &self,
        db: &mut dyn Driver,
        header: &Fields,
        parts: &BTreeMap<String, Vec<Fields>>,
    ) -> Result<Document, CoreError> {
        if let Some(unknown) = parts
            .keys()
            .find(|k| !self.parts.iter().any(|p| &p.name == *k))
        {
            return Err(CoreError::Invalid(format!(
                "в {} нет табличной части {unknown}",
                self.table
            )));
        }

        let mut values = vec![Value::Int(1), Value::Int(0)];
        values.extend(to_values(&self.table, &self.header, header)?);
        let d = db.dialect();
        let mut cols = vec![d.quote_ident("version"), d.quote_ident("posted")];
        cols.extend(names(d, &self.header));
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
            d.quote_ident(&self.table),
            cols.join(", "),
            placeholders(d, 1, cols.len()),
            d.quote_ident("id"),
        );
        let id = returned_id(&db.query(&sql, &values)?)?;

        for part in &self.parts {
            let lines = parts.get(&part.name).map(Vec::as_slice).unwrap_or_default();
            self.insert_lines(db, part, id, lines)?;
        }
        self.get(db, id)?.ok_or_else(|| self.not_found(id))
    }

    pub fn get(&self, db: &mut dyn Driver, id: i64) -> Result<Option<Document>, CoreError> {
        let d = db.dialect();
        let mut cols = vec![
            d.quote_ident("id"),
            d.quote_ident("version"),
            d.quote_ident("posted"),
        ];
        cols.extend(names(d, &self.header));
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = {}",
            cols.join(", "),
            d.quote_ident(&self.table),
            d.quote_ident("id"),
            d.placeholder(1),
        );
        let Some(row) = db.query(&sql, &[Value::Int(id)])?.into_iter().next() else {
            return Ok(None);
        };
        let mut values = row.into_iter();
        let (Some(Value::Int(id)), Some(Value::Int(version)), Some(Value::Int(posted))) =
            (values.next(), values.next(), values.next())
        else {
            return Err(CoreError::Invalid(format!(
                "в {} битая строка документа",
                self.table
            )));
        };
        let header = read_fields(&self.header, values)?;

        let mut parts = BTreeMap::new();
        for part in &self.parts {
            parts.insert(part.name.clone(), self.lines(db, part, id)?);
        }
        Ok(Some(Document {
            id,
            version,
            posted: posted != 0,
            header,
            parts,
        }))
    }

    pub(crate) fn set_posted(
        &self,
        db: &mut dyn Driver,
        id: i64,
        posted: bool,
    ) -> Result<(), CoreError> {
        let d = db.dialect();
        let version = d.quote_ident("version");
        let sql = format!(
            "UPDATE {} SET {} = {}, {version} = {version} + 1 WHERE {} = {}",
            d.quote_ident(&self.table),
            d.quote_ident("posted"),
            d.placeholder(1),
            d.quote_ident("id"),
            d.placeholder(2),
        );
        if db.execute(&sql, &[Value::Int(posted.into()), Value::Int(id)])? == 0 {
            return Err(self.not_found(id));
        }
        Ok(())
    }

    pub(crate) fn not_found(&self, id: i64) -> CoreError {
        CoreError::NotFound {
            table: self.table.clone(),
            id,
        }
    }

    fn part_table(&self, part: &TablePart) -> String {
        format!("{}__{}", self.table, part.name)
    }

    fn insert_lines(
        &self,
        db: &mut dyn Driver,
        part: &TablePart,
        doc_id: i64,
        lines: &[Fields],
    ) -> Result<(), CoreError> {
        let table = self.part_table(part);
        let d = db.dialect();
        let mut cols = vec![d.quote_ident("doc_id"), d.quote_ident("line_no")];
        cols.extend(names(d, &part.fields));
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            d.quote_ident(&table),
            cols.join(", "),
            placeholders(d, 1, cols.len()),
        );
        for (line_no, line) in (1..).zip(lines) {
            let mut values = vec![Value::Int(doc_id), Value::Int(line_no)];
            values.extend(to_values(&table, &part.fields, line)?);
            db.execute(&sql, &values)?;
        }
        Ok(())
    }

    fn lines(
        &self,
        db: &mut dyn Driver,
        part: &TablePart,
        doc_id: i64,
    ) -> Result<Vec<Fields>, CoreError> {
        let d = db.dialect();
        let mut cols = names(d, &part.fields);
        if cols.is_empty() {
            cols.push(d.quote_ident("line_no"));
        }
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = {} ORDER BY {}",
            cols.join(", "),
            d.quote_ident(&self.part_table(part)),
            d.quote_ident("doc_id"),
            d.placeholder(1),
            d.quote_ident("line_no"),
        );
        db.query(&sql, &[Value::Int(doc_id)])?
            .into_iter()
            .map(|row| read_fields(&part.fields, row.into_iter()))
            .collect()
    }
}
