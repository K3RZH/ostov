use serde::Deserialize;

/// Тип поля из строки `type`: `int`, `text`, `date`, `decimal(N)`, `ref(X)`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum FieldType {
    Int,
    Text,
    Date,
    /// `scale` — знаков после запятой.
    Decimal {
        scale: u8,
    },
    /// Имя объекта, на который ссылается поле. Существует ли он, проверяет `ostov check`.
    Ref(String),
}

const SUPPORTED: &str = "int, text, date, decimal(N), ref(X)";

impl TryFrom<String> for FieldType {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        match text.as_str() {
            "int" => return Ok(Self::Int),
            "text" => return Ok(Self::Text),
            "date" => return Ok(Self::Date),
            "decimal" => {
                return Err(
                    "у decimal не указана точность: пиши decimal(N), где N — знаков после запятой"
                        .into(),
                );
            }
            "ref" => return Err("у ref не указан объект: пиши ref(имя)".into()),
            _ => {}
        }
        if let Some(scale) = call_argument(&text, "decimal") {
            return scale
                .parse()
                .map(|scale| Self::Decimal { scale })
                .map_err(|_| {
                    format!("точность decimal({scale}) должна быть целым числом от 0 до 255")
                });
        }
        if let Some(target) = call_argument(&text, "ref") {
            if target.is_empty() {
                return Err("у ref не указан объект: пиши ref(имя)".into());
            }
            return Ok(Self::Ref(target.to_owned()));
        }
        Err(format!("неизвестный тип «{text}»: допустимы {SUPPORTED}"))
    }
}

/// `decimal(3)` → `Some("3")` для `name = "decimal"`.
fn call_argument<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<FieldType, String> {
        FieldType::try_from(text.to_owned())
    }

    #[test]
    fn parses_every_supported_type() {
        assert_eq!(parse("int"), Ok(FieldType::Int));
        assert_eq!(parse("text"), Ok(FieldType::Text));
        assert_eq!(parse("date"), Ok(FieldType::Date));
        assert_eq!(parse("decimal(3)"), Ok(FieldType::Decimal { scale: 3 }));
        assert_eq!(parse("ref(goods)"), Ok(FieldType::Ref("goods".into())));
    }

    #[test]
    fn decimal_without_scale_is_rejected() {
        let error = parse("decimal").unwrap_err();
        assert!(error.contains("не указана точность"), "{error}");
    }

    #[test]
    fn malformed_types_are_rejected() {
        for text in [
            "decimal(x)",
            "decimal(-1)",
            "decimal()",
            "ref()",
            "ref",
            "string",
            "Ref(goods)",
            "decimal(3",
        ] {
            assert!(parse(text).is_err(), "{text} должен быть ошибкой");
        }
    }
}
