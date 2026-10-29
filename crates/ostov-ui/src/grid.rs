//! Ширины колонок таблицы при перетаскивании границы (ErpGrid). Чистая логика без интерфейса.
//!
//! Правило: колонка, за правую границу которой тянут, меняет свою ширину, а место берётся у
//! колонок справа по очереди, пока они не упрутся в минимальную ширину. Так, чтобы расширить
//! одну колонку из многих, не нужно двигать все колонки перед ней.

/// Минимальная ширина обычной колонки, px.
pub const MIN_WIDTH: f32 = 48.0;

/// Сдвигает правую границу колонки `index` на `delta` px (положительное: вправо).
///
/// `widths`: заданные ширины колонок; у гибкой колонки (`flex`) это её минимум, а фактическую
/// ширину (`flex_actual`) определяет остаток места таблицы. Результат пишется в `widths`.
pub fn resize(widths: &mut [f32], flex: Option<usize>, flex_actual: f32, index: usize, delta: f32) {
    if index >= widths.len() || delta == 0.0 {
        return;
    }
    let is_flex = |j: usize| flex == Some(j);
    // Сколько колонка может отдать, сжавшись до минимума. Гибкая отдаёт избыток над минимумом.
    let slack = |widths: &[f32], j: usize| {
        if is_flex(j) {
            (flex_actual - widths[j]).max(0.0)
        } else {
            (widths[j] - MIN_WIDTH).max(0.0)
        }
    };
    if delta > 0.0 {
        // Порядок, в котором соседи отдают место: гибкая колонка справа раньше всех (она для
        // этого и гибкая), затем остальные колонки справа по порядку от границы.
        let mut order: Vec<usize> = Vec::new();
        if let Some(f) = flex.filter(|f| *f > index) {
            order.push(f);
        }
        order.extend((index + 1..widths.len()).filter(|j| !is_flex(*j)));
        let mut need = delta;
        for j in order {
            if need <= 0.0 {
                break;
            }
            let take = need.min(slack(widths, j));
            if !is_flex(j) {
                widths[j] -= take;
            }
            need -= take;
        }
        // Сколько удалось взять, настолько и выросла колонка.
        if !is_flex(index) {
            widths[index] += delta - need;
        }
    } else {
        let amount = -delta;
        let give = amount.min(slack(widths, index));
        if !is_flex(index) {
            widths[index] -= give;
        }
        // Освободившееся место получает гибкая колонка справа (сама, остатком) или соседняя.
        let flex_on_right = flex.is_some_and(|f| f > index);
        if !flex_on_right && index + 1 < widths.len() {
            widths[index + 1] += give;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_pushes_neighbours_until_minimum() {
        // Без гибкой колонки: [100, 100, 100], тянем границу первой на +80.
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, None, 0.0, 0, 40.0);
        assert_eq!(w, [140.0, 60.0, 100.0]);
    }

    #[test]
    fn widening_continues_to_next_columns() {
        // Сосед исчерпан (52 запаса), остаток берётся у следующего.
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, None, 0.0, 0, 70.0);
        assert_eq!(w, [170.0, MIN_WIDTH, 82.0]);
    }

    #[test]
    fn widening_stops_when_everything_is_minimal() {
        let mut w = [100.0, 60.0, 60.0];
        resize(&mut w, None, 0.0, 0, 500.0);
        assert_eq!(w, [100.0 + 24.0, MIN_WIDTH, MIN_WIDTH]);
    }

    #[test]
    fn flex_on_the_right_absorbs_first() {
        // [A, гибкая (мин. 100, сейчас 300), C]: расширение A забирает у гибкой.
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, Some(1), 300.0, 0, 150.0);
        assert_eq!(w, [250.0, 100.0, 100.0]);
        // Больше запаса у гибкой нет: дальше сжимается C.
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, Some(1), 300.0, 0, 230.0);
        assert_eq!(w, [330.0, 100.0, 70.0]);
    }

    #[test]
    fn narrowing_gives_space_to_flex_or_neighbour() {
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, Some(1), 300.0, 0, -40.0);
        assert_eq!(w, [60.0, 100.0, 100.0]);
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, None, 0.0, 0, -40.0);
        assert_eq!(w, [60.0, 140.0, 100.0]);
    }

    #[test]
    fn narrowing_stops_at_minimum() {
        let mut w = [60.0, 100.0];
        resize(&mut w, None, 0.0, 0, -100.0);
        assert_eq!(w, [MIN_WIDTH, 112.0]);
    }

    #[test]
    fn dragging_the_flex_column_moves_its_right_neighbour() {
        // Гибкая колонка: её граница двигается, соседи справа отдают место.
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, Some(1), 300.0, 1, 40.0);
        assert_eq!(w, [100.0, 100.0, 60.0]);
        // Влево: гибкая сужается, соседняя справа растёт.
        let mut w = [100.0, 100.0, 100.0];
        resize(&mut w, Some(1), 300.0, 1, -50.0);
        assert_eq!(w, [100.0, 100.0, 150.0]);
    }
}
