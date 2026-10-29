use ostov_store::Value;

use crate::CoreError;

/// Где закончилось окно: id последней строки и значение поля сортировки.
/// Клиент получает его строкой и только передаёт обратно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub id: i64,
    /// `None` — сортировка по id; `Some(Value::Null)` — у строки пустое поле сортировки.
    pub value: Option<Value>,
}

impl Cursor {
    pub fn encode(&self) -> String {
        match &self.value {
            None => self.id.to_string(),
            Some(Value::Null) => format!("{}|n", self.id),
            Some(Value::Int(v)) => format!("{}|i{v}", self.id),
            Some(Value::Text(v)) => format!("{}|t{v}", self.id),
            // Bytes не бывают полем сортировки: таких полей у объектов нет.
            Some(Value::Bytes(_)) => format!("{}|n", self.id),
        }
    }

    pub fn decode(key: &str) -> Result<Self, CoreError> {
        let bad = || CoreError::Invalid("испорченный ключ страницы списка".into());
        let (id, rest) = match key.split_once('|') {
            Some((id, rest)) => (id, Some(rest)),
            None => (key, None),
        };
        let id = id.parse().map_err(|_| bad())?;
        let value = match rest {
            None => None,
            Some("n") => Some(Value::Null),
            Some(rest) => match rest.split_at_checked(1) {
                Some(("i", v)) => Some(Value::Int(v.parse().map_err(|_| bad())?)),
                Some(("t", v)) => Some(Value::Text(v.to_owned())),
                _ => return Err(bad()),
            },
        };
        Ok(Self { id, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for cursor in [
            Cursor { id: 7, value: None },
            Cursor {
                id: 7,
                value: Some(Value::Null),
            },
            Cursor {
                id: -1,
                value: Some(Value::Int(-1250)),
            },
            Cursor {
                id: 3,
                value: Some(Value::Text("Мука | высший сорт".into())),
            },
            Cursor {
                id: 3,
                value: Some(Value::Text(String::new())),
            },
        ] {
            assert_eq!(Cursor::decode(&cursor.encode()).unwrap(), cursor);
        }
    }

    #[test]
    fn rejects_garbage() {
        for key in ["", "x", "1|", "1|q5", "1|iабв", "1.5"] {
            assert!(Cursor::decode(key).is_err(), "{key:?}");
        }
    }
}
