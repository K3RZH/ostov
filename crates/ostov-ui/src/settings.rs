//! Настройки интерфейса: тема, скругление, плотность строк таблицы, размер шрифта.
//! Хранятся в `~/.config/ostov/ui.conf` строками `ключ=значение`,
//! чтобы не тянуть библиотеку для разбора.

use std::path::PathBuf;

/// Скругление контролов по режимам: строгое, обычное, мягкое, капсула (px).
const RADIUS_PX: [f32; 4] = [4.0, 6.0, 8.0, 22.0];
/// Высота строки таблицы по режимам: плотная, обычная, просторная (px).
const ROW_PX: [f32; 3] = [28.0, 32.0, 40.0];
/// Масштаб шрифта по режимам: мелкий (12 px), обычный (14 px), крупный (16 px) для основного текста.
const FONT_SCALE: [f32; 3] = [12.0 / 14.0, 1.0, 16.0 / 14.0];
/// Режим по умолчанию у всех настроек, кроме темы: обычное.
const DEFAULT_MODE: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub dark: bool,
    /// Режим скругления (0..=3).
    pub radius: i32,
    /// Режим плотности строк (0..=2).
    pub density: i32,
    /// Режим размера шрифта (0..=2).
    pub font: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: false,
            radius: DEFAULT_MODE,
            density: DEFAULT_MODE,
            font: DEFAULT_MODE,
        }
    }
}

/// Значение режима из текста, если оно в пределах `0..count`.
fn mode(value: &str, count: usize) -> Option<i32> {
    value
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|mode| usize::try_from(*mode).is_ok_and(|m| m < count))
}

fn pick(table: &[f32], mode: i32) -> f32 {
    usize::try_from(mode)
        .ok()
        .and_then(|i| table.get(i).copied())
        .unwrap_or(table[usize::try_from(DEFAULT_MODE).unwrap_or(0)])
}

impl Settings {
    /// Радиус контролов в px для текущего режима.
    pub fn radius_px(self) -> f32 {
        pick(&RADIUS_PX, self.radius)
    }

    /// Высота строки таблицы в px.
    pub fn row_px(self) -> f32 {
        pick(&ROW_PX, self.density)
    }

    /// Масштаб шрифта относительно обычного.
    pub fn font_scale(self) -> f32 {
        pick(&FONT_SCALE, self.font)
    }

    /// Разбирает текст файла; неизвестные и битые строки пропускает.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "theme" => settings.dark = value.trim() == "dark",
                "radius" => {
                    settings.radius = mode(value, RADIUS_PX.len()).unwrap_or(settings.radius)
                }
                "density" => {
                    settings.density = mode(value, ROW_PX.len()).unwrap_or(settings.density)
                }
                "font" => settings.font = mode(value, FONT_SCALE.len()).unwrap_or(settings.font),
                _ => {}
            }
        }
        settings
    }

    pub fn to_text(self) -> String {
        format!(
            "theme={}\nradius={}\ndensity={}\nfont={}\n",
            if self.dark { "dark" } else { "light" },
            self.radius,
            self.density,
            self.font
        )
    }

    /// Читает настройки; нет файла или он битый: значения по умолчанию.
    pub fn load() -> Self {
        path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Сохраняет настройки; ошибку записи не показываем: настройки не критичны.
    pub fn save(self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("ostov").join("ui.conf"))
}
