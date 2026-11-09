//! Ключи сортировки списка по нескольким колонкам. Не знает, что за строки в списке: колонка это
//! её номер, а порядок строк считает ядро (или тот, кто сравнивает строки по этим ключам).

/// Сортировка списка по нескольким колонкам. Порядок ключей это приоритет: он такой, в каком
/// пользователь включал сортировку. У колонки три состояния: без сортировки, по возрастанию,
/// по убыванию (щелчок по заголовку переключает по кругу, меню заголовка задаёт прямо).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sorting {
    keys: Vec<(usize, bool)>,
}

impl Sorting {
    /// Ключи по приоритету: колонка и признак «по убыванию».
    pub fn keys(&self) -> &[(usize, bool)] {
        &self.keys
    }

    fn position(&self, column: usize) -> Option<usize> {
        self.keys.iter().position(|(c, _)| *c == column)
    }

    /// Состояние колонки: 0 нет, 1 по возрастанию, 2 по убыванию.
    pub fn dir(&self, column: usize) -> i32 {
        match self.position(column).map(|i| self.keys[i].1) {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }
    }

    /// Приоритет колонки с единицы; 0, если сортировки нет.
    pub fn priority(&self, column: usize) -> i32 {
        self.position(column)
            .map_or(0, |i| i32::try_from(i + 1).unwrap_or(0))
    }

    /// Щелчок по заголовку: нет → по возрастанию (в конец очереди) → по убыванию → нет.
    pub fn click(&mut self, column: usize) {
        match self.position(column) {
            None => self.keys.push((column, false)),
            Some(i) if !self.keys[i].1 => self.keys[i].1 = true,
            Some(i) => {
                self.keys.remove(i);
            }
        }
    }

    /// Задаёт направление; новая колонка встаёт в конец очереди, у включённой место не меняется.
    pub fn set(&mut self, column: usize, descending: bool) {
        match self.position(column) {
            Some(i) => self.keys[i].1 = descending,
            None => self.keys.push((column, descending)),
        }
    }

    pub fn clear(&mut self, column: usize) {
        if let Some(i) = self.position(column) {
            self.keys.remove(i);
        }
    }

    pub fn clear_all(&mut self) {
        self.keys.clear();
    }

    /// Меняет приоритет: `earlier` ставит колонку выше (раньше), иначе ниже.
    pub fn shift(&mut self, column: usize, earlier: bool) {
        let Some(i) = self.position(column) else {
            return;
        };
        let j = if earlier {
            i.checked_sub(1)
        } else {
            Some(i + 1).filter(|j| *j < self.keys.len())
        };
        if let Some(j) = j {
            self.keys.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_cycles_through_three_states() {
        let mut sorting = Sorting::default();
        assert_eq!(sorting.dir(2), 0);
        sorting.click(2);
        assert_eq!(sorting.dir(2), 1);
        sorting.click(2);
        assert_eq!(sorting.dir(2), 2);
        sorting.click(2);
        assert_eq!(sorting.dir(2), 0);
        assert!(sorting.keys().is_empty());
    }

    #[test]
    fn priority_follows_the_order_of_setting() {
        let mut sorting = Sorting::default();
        sorting.click(5);
        sorting.click(2);
        sorting.click(1);
        assert_eq!(
            [5, 2, 1].map(|c| sorting.priority(c)),
            [1, 2, 3],
            "приоритет равен порядку включения"
        );
        // Повторный щелчок по включённой колонке место в очереди не меняет.
        sorting.click(5);
        assert_eq!((sorting.priority(5), sorting.dir(5)), (1, 2));
        sorting.shift(1, true);
        assert_eq!(
            sorting.keys().iter().map(|k| k.0).collect::<Vec<_>>(),
            [5, 1, 2]
        );
        sorting.shift(5, true);
        sorting.shift(2, false);
        assert_eq!(
            sorting.keys().iter().map(|k| k.0).collect::<Vec<_>>(),
            [5, 1, 2]
        );
        sorting.clear(1);
        assert_eq!(sorting.priority(2), 2);
        sorting.set(0, true);
        assert_eq!((sorting.priority(0), sorting.dir(0)), (3, 2));
        sorting.clear_all();
        assert!(sorting.keys().is_empty());
    }
}
