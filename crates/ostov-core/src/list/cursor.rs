use ostov_store::Value;

use crate::CoreError;

/// Где закончилось окно: id последней строки и значения ключей сортировки.
/// Клиент получает его строкой и только передаёт обратно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub id: i64,
    /// По ключу сортировки на значение; пусто — сортировка по id.
    pub values: Vec<Value>,
}

impl Cursor {
    /// `id`, затем `|n`, `|i<число>` или `|t<длина>:<текст>` на каждый ключ:
    /// длина в байтах, поэтому `|` внутри текста не мешает.
    pub fn encode(&self) -> String {
        let mut key = self.id.to_string();
        for value in &self.values {
            match value {
                Value::Int(v) => key.push_str(&format!("|i{v}")),
                Value::Text(v) => key.push_str(&format!("|t{}:{v}", v.len())),
                // Bytes не бывают полем сортировки: таких полей у объектов нет.
                Value::Null | Value::Bytes(_) => key.push_str("|n"),
            }
        }
        key
    }

    pub fn decode(key: &str) -> Result<Self, CoreError> {
        let bad = || CoreError::Invalid("испорченный ключ страницы списка".into());
        let (id, mut rest) = key
            .split_once('|')
            .map_or((key, ""), |(id, rest)| (id, rest));
        let id = id.parse().map_err(|_| bad())?;
        let mut values = Vec::new();
        if key.contains('|') {
            loop {
                let (value, tail) = match rest.split_at_checked(1) {
                    Some(("n", tail)) => (Value::Null, tail),
                    Some(("i", tail)) => {
                        let end = tail.find('|').unwrap_or(tail.len());
                        let n = tail[..end].parse().map_err(|_| bad())?;
                        (Value::Int(n), &tail[end..])
                    }
                    Some(("t", tail)) => {
                        let (len, text) = tail.split_once(':').ok_or_else(bad)?;
                        let len: usize = len.parse().map_err(|_| bad())?;
                        let value = text.get(..len).ok_or_else(bad)?;
                        (Value::Text(value.to_owned()), &text[len..])
                    }
                    _ => return Err(bad()),
                };
                values.push(value);
                match tail.strip_prefix('|') {
                    Some(next) => rest = next,
                    None if tail.is_empty() => break,
                    None => return Err(bad()),
                }
            }
        }
        Ok(Self { id, values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for values in [
            vec![],
            vec![Value::Null],
            vec![Value::Int(-1250)],
            vec![Value::Text("Мука | высший сорт".into())],
            vec![Value::Text(String::new())],
            vec![
                Value::Text("a|t3:b".into()),
                Value::Null,
                Value::Int(7),
                Value::Text("2026-01-01".into()),
            ],
        ] {
            let cursor = Cursor { id: 7, values };
            assert_eq!(Cursor::decode(&cursor.encode()).unwrap(), cursor);
        }
    }

    #[test]
    fn rejects_garbage() {
        for key in [
            "",
            "x",
            "1|",
            "1|q5",
            "1|iабв",
            "1.5",
            "1|t5:ab",
            "1|t2:abc",
            "1|n|",
            "1|nn",
            "1|t:x",
        ] {
            assert!(Cursor::decode(key).is_err(), "{key:?}");
        }
    }
}
