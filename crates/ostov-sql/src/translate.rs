//! Проверка AST по белому списку и вывод SQL под диалект.
//!
//! SQL собирается заново из разобранного дерева, текст запроса в выход не копируется.
//! Структуры sqlparser разбираются целиком, без `..`: новое поле в следующей версии
//! sqlparser не пройдёт молча, а остановит сборку.

use std::collections::{BTreeMap, BTreeSet};

use ostov_model::{Config, FieldKind};
use ostov_store::{Dialect, MAX_SCALE, Value};
use sqlparser::ast::{
    self, BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr,
    Ident, JoinConstraint, JoinOperator, LimitClause, ObjectName, ObjectNamePart, OrderByKind,
    OrderBySort, SelectFlavor, SelectItem, SetExpr, Spanned, TableFactor, UnaryOperator,
};

use crate::error::Pos;
use crate::names::{self, Table};
use crate::{Column, MAX_DEPTH, MAX_LIMIT, Param, Prepared, SqlError};

struct Source {
    /// Как к таблице обращаются в запросе: псевдоним или имя метаданных.
    name: String,
    /// Псевдоним в выходном SQL: `t1`, `t2`, … — пользовательские имена туда не попадают.
    out: String,
    table: Table,
}

type Typed = (String, Option<FieldKind>);

/// Числовой параметр в выходных параметрах: его можно перевести в единицы денежного поля.
struct Number {
    index: usize,
    name: String,
    scale: u8,
}

pub(crate) fn run(
    query: ast::Query,
    params: &BTreeMap<String, Param>,
    config: &Config,
    can_read: &dyn Fn(&str) -> bool,
    dialect: &dyn Dialect,
) -> Result<Prepared, SqlError> {
    let mut t = Translator {
        dialect,
        params,
        used: BTreeSet::new(),
        out_params: Vec::new(),
        numbers: BTreeMap::new(),
        sources: Vec::new(),
        aliases: Vec::new(),
        literals: BTreeMap::new(),
        depth: 0,
    };
    let (sql, columns) = t.query(query, config, can_read)?;
    if let Some(extra) = params.keys().find(|k| !t.used.contains(*k)) {
        return Err(SqlError::ExtraParam {
            name: extra.clone(),
        });
    }
    Ok(Prepared {
        sql,
        params: t.out_params,
        columns,
    })
}

fn forbidden(what: &str, span: sqlparser::tokenizer::Span) -> SqlError {
    SqlError::Forbidden {
        what: what.to_owned(),
        at: Pos::from_span(span),
    }
}

fn no_pos(what: &str) -> SqlError {
    SqlError::Forbidden {
        what: what.to_owned(),
        at: None,
    }
}

/// Имя без кавычек, в нижнем регистре: в метаданных имена только `[a-z][a-z0-9_]*`.
fn ident(id: &Ident) -> Result<String, SqlError> {
    if id.quote_style.is_some() {
        return Err(SqlError::QuotedName {
            name: id.value.clone(),
            at: Pos::from_span(id.span),
        });
    }
    Ok(id.value.to_ascii_lowercase())
}

fn object_name(name: &ObjectName) -> Result<Vec<String>, SqlError> {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(id) => ident(id),
            ObjectNamePart::Function(_) => Err(forbidden("функция в имени", name.span())),
        })
        .collect()
}

struct Translator<'a> {
    dialect: &'a dyn Dialect,
    params: &'a BTreeMap<String, Param>,
    used: BTreeSet<String>,
    out_params: Vec<Value>,
    /// Плейсхолдер числового параметра → где его значение.
    numbers: BTreeMap<String, Number>,
    sources: Vec<Source>,
    /// Псевдонимы колонок результата: на них можно ссылаться в ORDER BY.
    aliases: Vec<String>,
    /// Одинаковые строковые литералы — один плейсхолдер: иначе в PostgreSQL
    /// `GROUP BY COALESCE(name, '—')` не совпадёт с тем же выражением в SELECT.
    literals: BTreeMap<String, String>,
    depth: usize,
}

impl Translator<'_> {
    fn query(
        &mut self,
        query: ast::Query,
        config: &Config,
        can_read: &dyn Fn(&str) -> bool,
    ) -> Result<(String, Vec<Column>), SqlError> {
        let ast::Query {
            with,
            body,
            order_by,
            limit_clause,
            fetch,
            locks,
            for_clause,
            settings,
            format_clause,
            pipe_operators,
        } = query;
        if let Some(with) = with {
            return Err(forbidden("WITH (общие табличные выражения)", with.span()));
        }
        if fetch.is_some() {
            return Err(no_pos("FETCH"));
        }
        if !locks.is_empty() {
            return Err(no_pos("FOR UPDATE / FOR SHARE"));
        }
        if for_clause.is_some() || settings.is_some() || format_clause.is_some() {
            return Err(no_pos("FOR, SETTINGS, FORMAT"));
        }
        if !pipe_operators.is_empty() {
            return Err(no_pos("конвейер |>"));
        }
        let select = match *body {
            SetExpr::Select(s) => s,
            SetExpr::SetOperation { .. } => {
                return Err(no_pos("UNION, INTERSECT, EXCEPT"));
            }
            SetExpr::Query(q) => return Err(forbidden("подзапрос", q.span())),
            SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => {
                return Err(SqlError::OnlySelect);
            }
            _ => return Err(no_pos("VALUES и TABLE вместо SELECT")),
        };
        let ast::Select {
            select_token: _,
            optimizer_hints,
            distinct,
            select_modifiers,
            top,
            top_before_distinct: _,
            projection,
            exclude,
            into,
            from,
            lateral_views,
            prewhere,
            selection,
            connect_by,
            group_by,
            cluster_by,
            distribute_by,
            sort_by,
            having,
            named_window,
            qualify,
            window_before_qualify: _,
            value_table_mode,
            flavor,
        } = *select;
        if !optimizer_hints.is_empty() {
            return Err(no_pos("подсказки оптимизатору"));
        }
        if distinct.is_some() {
            return Err(no_pos("DISTINCT"));
        }
        if select_modifiers.is_some() || top.is_some() {
            return Err(no_pos("модификаторы SELECT и TOP"));
        }
        if exclude.is_some() {
            return Err(no_pos("EXCLUDE"));
        }
        if into.is_some() {
            return Err(no_pos("SELECT INTO"));
        }
        if !lateral_views.is_empty() || prewhere.is_some() || !connect_by.is_empty() {
            return Err(no_pos("LATERAL VIEW, PREWHERE, CONNECT BY"));
        }
        if !cluster_by.is_empty() || !distribute_by.is_empty() || !sort_by.is_empty() {
            return Err(no_pos("CLUSTER BY, DISTRIBUTE BY, SORT BY"));
        }
        if !named_window.is_empty() || qualify.is_some() {
            return Err(no_pos("WINDOW и QUALIFY"));
        }
        if value_table_mode.is_some() {
            return Err(no_pos("SELECT AS STRUCT / VALUE"));
        }
        if flavor != SelectFlavor::Standard {
            return Err(no_pos("FROM перед SELECT"));
        }

        // Сначала все таблицы и права, потом вывод: в SELECT ссылаются на таблицы из FROM.
        if from.len() > 1 {
            return Err(no_pos(
                "перечислять таблицы через запятую: соедините их через JOIN … ON",
            ));
        }
        let mut joins = Vec::new();
        if let Some(first) = from.into_iter().next() {
            self.add_source(&first.relation, config, can_read)?;
            for join in first.joins {
                let ast::Join {
                    relation,
                    global,
                    join_operator,
                } = join;
                if global {
                    return Err(no_pos("GLOBAL JOIN"));
                }
                self.add_source(&relation, config, can_read)?;
                joins.push(join_operator);
            }
        }

        let mut sql = String::from("SELECT ");
        let mut columns = Vec::new();
        for (i, item) in projection.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            let column = match item {
                SelectItem::UnnamedExpr(e) => {
                    let (text, kind) = self.expr(e)?;
                    sql.push_str(&text);
                    let (target, flag) = self.origin(e);
                    Column {
                        name: result_name(e, i),
                        kind,
                        target,
                        flag,
                    }
                }
                SelectItem::ExprWithAlias { expr, alias } => {
                    let (text, kind) = self.expr(expr)?;
                    let alias = ident(alias)?;
                    sql.push_str(&text);
                    self.aliases.push(alias.clone());
                    let (target, flag) = self.origin(expr);
                    Column {
                        name: alias,
                        kind,
                        target,
                        flag,
                    }
                }
                SelectItem::ExprWithAliases { expr, .. } => {
                    return Err(forbidden("несколько псевдонимов у колонки", expr.span()));
                }
                SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(..) => {
                    return Err(forbidden(
                        "SELECT *: перечислите нужные колонки явно",
                        item.span(),
                    ));
                }
            };
            // Имя всегда явно: иначе SQLite и PostgreSQL назовут выражение по-разному.
            if columns.iter().any(|c: &Column| c.name == column.name) {
                return Err(SqlError::DuplicateColumn { name: column.name });
            }
            sql.push_str(" AS ");
            sql.push_str(&self.dialect.quote_ident(&column.name));
            columns.push(column);
        }

        if !self.sources.is_empty() {
            sql.push_str(" FROM ");
            sql.push_str(&self.table_sql(0));
            for (i, op) in joins.iter().enumerate() {
                let (kind, constraint) = match op {
                    JoinOperator::Join(c) | JoinOperator::Inner(c) => ("INNER JOIN", c),
                    JoinOperator::Left(c) | JoinOperator::LeftOuter(c) => ("LEFT JOIN", c),
                    _ => return Err(no_pos("такой JOIN: можно только INNER и LEFT")),
                };
                let JoinConstraint::On(on) = constraint else {
                    return Err(no_pos("JOIN без ON (USING, NATURAL)"));
                };
                let on = self.expr(on)?.0;
                sql.push_str(&format!(" {kind} {} ON {on}", self.table_sql(i + 1)));
            }
        }

        if let Some(cond) = &selection {
            let cond = self.expr(cond)?.0;
            sql.push_str(" WHERE ");
            sql.push_str(&cond);
        }

        match &group_by {
            GroupByExpr::Expressions(exprs, modifiers) => {
                if !modifiers.is_empty() {
                    return Err(no_pos("ROLLUP, CUBE, GROUPING SETS"));
                }
                if !exprs.is_empty() {
                    let list = self.list(exprs)?;
                    sql.push_str(" GROUP BY ");
                    sql.push_str(&list);
                }
            }
            GroupByExpr::All(_) => return Err(no_pos("GROUP BY ALL")),
        }

        if let Some(cond) = &having {
            let cond = self.expr(cond)?.0;
            sql.push_str(" HAVING ");
            sql.push_str(&cond);
        }

        if let Some(order) = order_by {
            let ast::OrderBy { kind, interpolate } = order;
            if interpolate.is_some() {
                return Err(no_pos("INTERPOLATE"));
            }
            let OrderByKind::Expressions(items) = kind else {
                return Err(no_pos("ORDER BY ALL"));
            };
            let mut parts = Vec::new();
            for item in &items {
                let ast::OrderByExpr {
                    expr,
                    options,
                    with_fill,
                } = item;
                if with_fill.is_some() {
                    return Err(no_pos("WITH FILL"));
                }
                if options.nulls_first.is_some() {
                    return Err(forbidden("NULLS FIRST / NULLS LAST", expr.span()));
                }
                // Псевдоним колонки — только целым элементом: `ORDER BY total + 1`
                // в PostgreSQL псевдоним не видит.
                let alias = match expr {
                    Expr::Identifier(id) => Some(ident(id)?),
                    _ => None,
                };
                let mut text = match alias {
                    Some(a) if self.aliases.contains(&a) => self.dialect.quote_ident(&a),
                    _ => self.expr(expr)?.0,
                };
                match &options.sort {
                    None | Some(OrderBySort::Asc) => {}
                    Some(OrderBySort::Desc) => text.push_str(" DESC"),
                    Some(OrderBySort::Using(_)) => return Err(no_pos("ORDER BY … USING")),
                }
                parts.push(text);
            }
            sql.push_str(" ORDER BY ");
            sql.push_str(&parts.join(", "));
        }

        let limit = match limit_clause {
            None => MAX_LIMIT,
            Some(LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            }) => {
                if offset.is_some() {
                    return Err(no_pos("OFFSET"));
                }
                if !limit_by.is_empty() {
                    return Err(no_pos("LIMIT BY"));
                }
                match limit {
                    None => MAX_LIMIT,
                    Some(Expr::Value(v)) => match &v.value {
                        ast::Value::Number(n, false) => {
                            n.parse::<u64>().map_err(|_| SqlError::BadLimit)?
                        }
                        _ => return Err(SqlError::BadLimit),
                    },
                    Some(_) => return Err(SqlError::BadLimit),
                }
            }
            Some(LimitClause::OffsetCommaLimit { .. }) => return Err(no_pos("OFFSET")),
        };
        sql.push_str(&format!(" LIMIT {}", limit.min(MAX_LIMIT)));

        Ok((sql, columns))
    }

    fn add_source(
        &mut self,
        factor: &TableFactor,
        config: &Config,
        can_read: &dyn Fn(&str) -> bool,
    ) -> Result<(), SqlError> {
        let (name, alias) = match factor {
            TableFactor::Table {
                name,
                alias,
                args,
                with_hints,
                version,
                with_ordinality,
                partitions,
                json_path,
                sample,
                index_hints,
            } => {
                if args.is_some() {
                    return Err(forbidden("табличная функция", name.span()));
                }
                if !with_hints.is_empty()
                    || version.is_some()
                    || *with_ordinality
                    || !partitions.is_empty()
                    || json_path.is_some()
                    || sample.is_some()
                    || !index_hints.is_empty()
                {
                    return Err(forbidden(
                        "подсказки, версии и выборки у таблицы",
                        name.span(),
                    ));
                }
                (name, alias)
            }
            TableFactor::Derived { subquery, .. } => {
                return Err(forbidden("подзапрос", subquery.span()));
            }
            other => return Err(forbidden("такой источник в FROM", other.span())),
        };
        let parts = object_name(name)?;
        let table = names::resolve(config, &parts)?;
        if let Some((object, title)) = &table.right
            && !can_read(object)
        {
            return Err(SqlError::NoRight {
                title: title.clone(),
            });
        }
        let binding = match alias {
            Some(a) => {
                let ast::TableAlias {
                    explicit: _,
                    name,
                    columns,
                    at,
                } = a;
                if !columns.is_empty() || at.is_some() {
                    return Err(forbidden("список колонок у псевдонима", name.span));
                }
                ident(name)?
            }
            None => parts.join("."),
        };
        if self.sources.iter().any(|s| s.name == binding) {
            return Err(SqlError::DuplicateSource { name: binding });
        }
        let out = format!("t{}", self.sources.len() + 1);
        self.sources.push(Source {
            name: binding,
            out,
            table,
        });
        Ok(())
    }

    fn table_sql(&self, i: usize) -> String {
        let s = &self.sources[i];
        format!(
            "{} AS {}",
            self.dialect.quote_ident(&s.table.physical),
            self.dialect.quote_ident(&s.out)
        )
    }

    fn column_sql(&self, source: &Source, column: &str) -> String {
        format!(
            "{}.{}",
            self.dialect.quote_ident(&source.out),
            self.dialect.quote_ident(column)
        )
    }

    fn list(&mut self, exprs: &[Expr]) -> Result<String, SqlError> {
        let mut out = Vec::with_capacity(exprs.len());
        for e in exprs {
            out.push(self.expr(e)?.0);
        }
        Ok(out.join(", "))
    }

    fn param(&mut self, raw: &str) -> Result<Typed, SqlError> {
        let name = raw
            .strip_prefix(':')
            .filter(|n| is_param_name(n))
            .ok_or_else(|| SqlError::BadParam {
                text: raw.to_owned(),
            })?;
        let value = self
            .params
            .get(name)
            .ok_or_else(|| SqlError::MissingParam {
                name: name.to_owned(),
            })?;
        self.used.insert(name.to_owned());
        let (value, kind, scale) = match value {
            Param::Null => (Value::Null, None, None),
            Param::Int(n) => (Value::Int(*n), Some(FieldKind::Int), Some(0)),
            Param::Text(text) if text.contains('\0') => return Err(SqlError::NulInText),
            Param::Text(text) => (Value::Text(text.clone()), Some(FieldKind::Text), None),
            Param::Decimal { units, scale } => (
                Value::Int(*units),
                Some(FieldKind::Decimal { scale: *scale }),
                Some(*scale),
            ),
        };
        let placeholder = self.push_value(value);
        if let Some(scale) = scale {
            let number = Number {
                index: self.out_params.len() - 1,
                name: name.to_owned(),
                scale,
            };
            self.numbers.insert(placeholder.clone(), number);
        }
        Ok((placeholder, kind))
    }

    /// Две стороны сравнения или суммы: числовой параметр переводится в единицы
    /// денежного поля напротив, разные масштабы двух полей — ошибка.
    fn align(&mut self, a: &Typed, b: &Typed) -> Result<(Typed, Typed), SqlError> {
        let a_kind = self.rescale(&a.0, b.1)?.or(a.1);
        let b_kind = self.rescale(&b.0, a.1)?.or(b.1);
        if let (Some(FieldKind::Decimal { scale: l }), Some(FieldKind::Decimal { scale: r })) =
            (a_kind, b_kind)
            && l != r
        {
            return Err(SqlError::ScaleMismatch { left: l, right: r });
        }
        money(a_kind, b_kind)?;
        Ok(((a.0.clone(), a_kind), (b.0.clone(), b_kind)))
    }

    fn rescale(
        &mut self,
        text: &str,
        other: Option<FieldKind>,
    ) -> Result<Option<FieldKind>, SqlError> {
        let Some(FieldKind::Decimal { scale }) = other else {
            return Ok(None);
        };
        let Some(number) = self.numbers.get_mut(text) else {
            return Ok(None);
        };
        if number.scale != scale {
            let Some(Value::Int(units)) = self.out_params.get(number.index) else {
                return Ok(None);
            };
            let units =
                to_scale(*units, number.scale, scale).ok_or_else(|| SqlError::ParamScale {
                    name: number.name.clone(),
                    scale,
                })?;
            if let Some(slot) = self.out_params.get_mut(number.index) {
                *slot = Value::Int(units);
            }
            number.scale = scale;
        }
        Ok(Some(FieldKind::Decimal { scale }))
    }

    /// Колонка результата — само поле: на что ссылается и отметка ли это.
    /// Имя уже проверено в `expr`, здесь только ищем его таблицу.
    fn origin(&self, e: &Expr) -> (Option<String>, bool) {
        let found = match e {
            Expr::Identifier(id) => ident(id).ok().and_then(|name| {
                let mut tables = self
                    .sources
                    .iter()
                    .filter(|s| s.table.column(&name).is_some());
                let source = tables.next()?;
                tables.next().is_none().then_some((source, name))
            }),
            Expr::CompoundIdentifier(ids) => {
                let parts: Option<Vec<String>> = ids.iter().map(|id| ident(id).ok()).collect();
                parts.and_then(|parts| {
                    let (column, prefix) = parts.split_last()?;
                    let binding = prefix.join(".");
                    let source = self.sources.iter().find(|s| s.name == binding)?;
                    Some((source, column.clone()))
                })
            }
            _ => None,
        };
        match found {
            Some((source, name)) => (source.table.target(&name), source.table.is_flag(&name)),
            None => (None, false),
        }
    }

    fn literal_text(&mut self, text: &str) -> String {
        if let Some(placeholder) = self.literals.get(text) {
            return placeholder.clone();
        }
        let placeholder = self.push_value(Value::Text(text.to_owned()));
        self.literals.insert(text.to_owned(), placeholder.clone());
        placeholder
    }

    fn push_value(&mut self, value: Value) -> String {
        self.out_params.push(value);
        self.dialect.placeholder(self.out_params.len())
    }

    fn expr(&mut self, e: &Expr) -> Result<Typed, SqlError> {
        if self.depth >= MAX_DEPTH {
            return Err(SqlError::TooComplex);
        }
        self.depth += 1;
        let result = self.expr_inner(e);
        self.depth -= 1;
        result
    }

    fn expr_inner(&mut self, e: &Expr) -> Result<Typed, SqlError> {
        match e {
            Expr::Identifier(id) => {
                let name = ident(id)?;
                let found: Vec<&Source> = self
                    .sources
                    .iter()
                    .filter(|s| s.table.column(&name).is_some())
                    .collect();
                match found.as_slice() {
                    [s] => Ok((self.column_sql(s, &name), s.table.column(&name))),
                    [] => Err(SqlError::NoField {
                        field: name,
                        table: self
                            .sources
                            .iter()
                            .map(|s| s.table.title.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    }),
                    _ => Err(SqlError::Ambiguous { field: name }),
                }
            }
            Expr::CompoundIdentifier(ids) => {
                let parts = ids.iter().map(ident).collect::<Result<Vec<_>, _>>()?;
                let Some((column, prefix)) = parts.split_last() else {
                    return Err(forbidden("пустое имя", e.span()));
                };
                let binding = prefix.join(".");
                let source = self
                    .sources
                    .iter()
                    .find(|s| s.name == binding)
                    .ok_or(SqlError::NoSource { name: binding })?;
                let kind = source
                    .table
                    .column(column)
                    .ok_or_else(|| SqlError::NoField {
                        field: column.clone(),
                        table: source.table.title.clone(),
                    })?;
                Ok((self.column_sql(source, column), Some(kind)))
            }
            Expr::Value(v) => self.literal(&v.value, e),
            Expr::BinaryOp { left, op, right } => {
                let mut left = self.expr(left)?;
                let mut right = self.expr(right)?;
                if matches!(
                    op,
                    BinaryOperator::Eq
                        | BinaryOperator::NotEq
                        | BinaryOperator::Lt
                        | BinaryOperator::LtEq
                        | BinaryOperator::Gt
                        | BinaryOperator::GtEq
                        | BinaryOperator::Plus
                        | BinaryOperator::Minus
                ) {
                    (left, right) = self.align(&left, &right)?;
                }
                let ((l, lk), (r, rk)) = (left, right);
                let (sym, kind) = match op {
                    BinaryOperator::Eq => ("=", None),
                    BinaryOperator::NotEq => ("<>", None),
                    BinaryOperator::Lt => ("<", None),
                    BinaryOperator::LtEq => ("<=", None),
                    BinaryOperator::Gt => (">", None),
                    BinaryOperator::GtEq => (">=", None),
                    BinaryOperator::And => ("AND", None),
                    BinaryOperator::Or => ("OR", None),
                    BinaryOperator::Plus => ("+", add_kind(lk, rk)?),
                    BinaryOperator::Minus => ("-", add_kind(lk, rk)?),
                    BinaryOperator::Multiply => ("*", mul_kind(lk, rk, e)?),
                    other => {
                        return Err(forbidden(&format!("оператор {other}"), e.span()));
                    }
                };
                Ok((format!("({l} {sym} {r})"), kind))
            }
            Expr::UnaryOp { op, expr } => {
                let (inner, kind) = self.expr(expr)?;
                match op {
                    UnaryOperator::Not => Ok((format!("(NOT {inner})"), None)),
                    UnaryOperator::Minus => Ok((format!("(-{inner})"), kind)),
                    UnaryOperator::Plus => Ok((inner, kind)),
                    other => Err(forbidden(&format!("оператор {other}"), e.span())),
                }
            }
            Expr::Nested(inner) => {
                let (text, kind) = self.expr(inner)?;
                Ok((format!("({text})"), kind))
            }
            Expr::IsNull(inner) => {
                let text = self.expr(inner)?.0;
                Ok((format!("({text} IS NULL)"), None))
            }
            Expr::IsNotNull(inner) => {
                let text = self.expr(inner)?.0;
                Ok((format!("({text} IS NOT NULL)"), None))
            }
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let head = self.expr(expr)?;
                let mut out = Vec::with_capacity(list.len());
                for item in list {
                    let item = self.expr(item)?;
                    out.push(self.align(&head, &item)?.1.0);
                }
                let text = head.0;
                let items = out.join(", ");
                let not = if *negated { "NOT " } else { "" };
                Ok((format!("({text} {not}IN ({items}))"), None))
            }
            Expr::Between {
                expr,
                negated,
                low,
                high,
            } => {
                let head = self.expr(expr)?;
                let low = self.expr(low)?;
                let high = self.expr(high)?;
                let low = self.align(&head, &low)?.1.0;
                let high = self.align(&head, &high)?.1.0;
                let text = head.0;
                let not = if *negated { "NOT " } else { "" };
                Ok((format!("({text} {not}BETWEEN {low} AND {high})"), None))
            }
            Expr::Like {
                negated,
                any,
                expr,
                pattern,
                escape_char,
            } => {
                if *any || escape_char.is_some() {
                    return Err(forbidden("LIKE ANY и ESCAPE", e.span()));
                }
                let text = self.expr(expr)?.0;
                let pattern = self.expr(pattern)?.0;
                let not = if *negated { "NOT " } else { "" };
                Ok((format!("({text} {not}LIKE {pattern})"), None))
            }
            Expr::Function(f) => self.function(f, e),
            Expr::Subquery(_) | Expr::InSubquery { .. } | Expr::Exists { .. } => {
                Err(forbidden("подзапрос", e.span()))
            }
            other => Err(forbidden(
                &format!("выражение «{}»", short(&other.to_string())),
                e.span(),
            )),
        }
    }

    fn literal(&mut self, value: &ast::Value, e: &Expr) -> Result<Typed, SqlError> {
        match value {
            ast::Value::Number(text, false) => {
                let n: i64 = text
                    .parse()
                    .map_err(|_| SqlError::BadNumber { text: text.clone() })?;
                // 0 одинаков в любых единицах: его можно рядом с суммой.
                let kind = (n != 0).then_some(FieldKind::Int);
                Ok((n.to_string(), kind))
            }
            ast::Value::Number(text, true) => Err(SqlError::BadNumber {
                text: format!("{text}L"),
            }),
            // Строку из запроса тоже передаём параметром: в выходной SQL текст не склеивается.
            ast::Value::SingleQuotedString(s) if s.contains('\0') => Err(SqlError::NulInText),
            ast::Value::SingleQuotedString(s) => Ok((self.literal_text(s), Some(FieldKind::Text))),
            ast::Value::Null => Ok(("NULL".to_owned(), None)),
            ast::Value::Placeholder(raw) => self.param(raw),
            ast::Value::Boolean(_) => Err(forbidden(
                "TRUE и FALSE: флажки хранятся числом, пишите 1 и 0",
                e.span(),
            )),
            _ => Err(forbidden(
                "такая запись строки: можно только в одинарных кавычках",
                e.span(),
            )),
        }
    }

    fn function(&mut self, f: &ast::Function, e: &Expr) -> Result<Typed, SqlError> {
        let ast::Function {
            name,
            uses_odbc_syntax,
            parameters,
            args,
            within_group,
            filter,
            null_treatment,
            over,
        } = f;
        let at = Pos::from_span(e.span());
        let parts = object_name(name)?;
        let fname = match parts.as_slice() {
            [one] => one.to_ascii_uppercase(),
            _ => {
                return Err(SqlError::Function {
                    name: parts.join("."),
                    at,
                });
            }
        };
        if !matches!(
            fname.as_str(),
            "COUNT" | "SUM" | "MIN" | "MAX" | "COALESCE" | "LOWER"
        ) {
            return Err(SqlError::Function { name: fname, at });
        }
        if *uses_odbc_syntax
            || !matches!(parameters, FunctionArguments::None)
            || !within_group.is_empty()
            || filter.is_some()
            || null_treatment.is_some()
            || over.is_some()
        {
            return Err(forbidden("OVER, FILTER, WITHIN GROUP у функции", e.span()));
        }
        let list = match args {
            FunctionArguments::List(list) => list,
            FunctionArguments::Subquery(q) => return Err(forbidden("подзапрос", q.span())),
            FunctionArguments::None => {
                return Err(SqlError::FunctionArgs {
                    name: fname,
                    need: "нужны скобки с аргументами",
                });
            }
        };
        let ast::FunctionArgumentList {
            duplicate_treatment,
            args,
            clauses,
        } = list;
        if duplicate_treatment.is_some() || !clauses.is_empty() {
            return Err(forbidden(
                "DISTINCT, ORDER BY и другие уточнения внутри функции",
                e.span(),
            ));
        }

        if fname == "COUNT"
            && let [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)] = args.as_slice()
        {
            return Ok(("COUNT(*)".to_owned(), None));
        }
        let mut texts = Vec::with_capacity(args.len());
        let mut kinds = Vec::with_capacity(args.len());
        for arg in args {
            let FunctionArg::Unnamed(FunctionArgExpr::Expr(a)) = arg else {
                return Err(SqlError::FunctionArgs {
                    name: fname,
                    need: "аргументы — только выражения (звёздочка — только в COUNT(*))",
                });
            };
            let (text, kind) = self.expr(a)?;
            texts.push(text);
            kinds.push(kind);
        }
        let one = |need| {
            if texts.len() == 1 {
                Ok(())
            } else {
                Err(SqlError::FunctionArgs {
                    name: fname.clone(),
                    need,
                })
            }
        };
        match fname.as_str() {
            "COUNT" => {
                one("один аргумент")?;
                Ok((format!("COUNT({})", texts[0]), None))
            }
            // В PostgreSQL SUM(bigint) — numeric, драйвер его не читает: приводим к целому.
            // Переполнение тогда — ошибка базы, а не тихая потеря.
            "SUM" => {
                one("один аргумент")?;
                if let Some(k @ (FieldKind::Text | FieldKind::Date | FieldKind::Ref)) = kinds[0] {
                    return Err(SqlError::SumNotNumber {
                        kind: k.to_string(),
                    });
                }
                Ok((format!("CAST(SUM({}) AS BIGINT)", texts[0]), kinds[0]))
            }
            "MIN" | "MAX" => {
                one("один аргумент")?;
                Ok((format!("{fname}({})", texts[0]), kinds[0]))
            }
            "LOWER" => {
                one("один аргумент")?;
                Ok((self.dialect.lower(&texts[0]), Some(FieldKind::Text)))
            }
            _ => {
                if texts.len() < 2 {
                    return Err(SqlError::FunctionArgs {
                        name: fname,
                        need: "нужно не меньше двух аргументов",
                    });
                }
                let kind =
                    kinds
                        .iter()
                        .try_fold(None, |acc: Option<FieldKind>, k| match (acc, *k) {
                            (None, k) => Ok(k),
                            (Some(a), None) => Ok(Some(a)),
                            (Some(a), Some(b)) => add_kind(Some(a), Some(b)).map(|_| Some(a)),
                        })?;
                Ok((format!("COALESCE({})", texts.join(", ")), kind))
            }
        }
    }
}

fn is_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Имя колонки результата без AS: имя поля или функции.
fn result_name(e: &Expr, i: usize) -> String {
    match e {
        Expr::Identifier(id) => id.value.to_ascii_lowercase(),
        Expr::CompoundIdentifier(ids) => ids
            .last()
            .map(|id| id.value.to_ascii_lowercase())
            .unwrap_or_default(),
        Expr::Function(f) => f.name.to_string().to_ascii_lowercase(),
        _ => format!("column{}", i + 1),
    }
}

/// Сумма рядом с целым полем или ненулевым числом: единицы разного смысла.
fn money(a: Option<FieldKind>, b: Option<FieldKind>) -> Result<(), SqlError> {
    match (a, b) {
        (Some(FieldKind::Decimal { scale }), Some(FieldKind::Int))
        | (Some(FieldKind::Int), Some(FieldKind::Decimal { scale })) => {
            Err(SqlError::MoneyWithInt { scale })
        }
        _ => Ok(()),
    }
}

/// Сумма или разность: деньги складываются в единицах одного масштаба.
fn add_kind(l: Option<FieldKind>, r: Option<FieldKind>) -> Result<Option<FieldKind>, SqlError> {
    match (l, r) {
        (Some(FieldKind::Decimal { scale: a }), Some(FieldKind::Decimal { scale: b })) => {
            if a == b {
                Ok(Some(FieldKind::Decimal { scale: a }))
            } else {
                Err(SqlError::ScaleMismatch { left: a, right: b })
            }
        }
        (Some(k @ FieldKind::Decimal { .. }), _) | (_, Some(k @ FieldKind::Decimal { .. })) => {
            Ok(Some(k))
        }
        _ => Ok(None),
    }
}

/// Произведение единиц: масштабы складываются (количество 0,001 × цена 0,01 → 0,00001).
fn mul_kind(
    l: Option<FieldKind>,
    r: Option<FieldKind>,
    e: &Expr,
) -> Result<Option<FieldKind>, SqlError> {
    match (l, r) {
        (Some(FieldKind::Decimal { scale: a }), Some(FieldKind::Decimal { scale: b })) => {
            let scale = a.saturating_add(b);
            if scale > MAX_SCALE {
                return Err(forbidden(
                    "произведение с таким числом знаков после запятой",
                    e.span(),
                ));
            }
            Ok(Some(FieldKind::Decimal { scale }))
        }
        (Some(k @ FieldKind::Decimal { .. }), _) | (_, Some(k @ FieldKind::Decimal { .. })) => {
            Ok(Some(k))
        }
        _ => Ok(None),
    }
}

fn short(text: &str) -> String {
    const MAX: usize = 40;
    if text.chars().count() <= MAX {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(MAX).collect();
        format!("{cut}…")
    }
}

/// Единицы из одного масштаба в другой; `None`, если теряются знаки или не влезает.
fn to_scale(units: i64, from: u8, to: u8) -> Option<i64> {
    if to >= from {
        units.checked_mul(10_i64.checked_pow(u32::from(to - from))?)
    } else {
        let div = 10_i64.checked_pow(u32::from(from - to))?;
        (units % div == 0).then_some(units / div)
    }
}
