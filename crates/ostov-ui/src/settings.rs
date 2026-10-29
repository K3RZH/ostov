//! Настройки интерфейса: тема и скругление. Хранятся в `~/.config/ostov/ui.conf`
//! строками `ключ=значение`, чтобы не тянуть библиотеку для разбора.

use std::path::PathBuf;

/// Скругление контролов по режимам: строгое, обычное, мягкое, капсула (px).
const RADIUS_PX: [f32; 4] = [4.0, 6.0, 8.0, 22.0];
/// Режим по умолчанию: обычное.
const DEFAULT_RADIUS: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub dark: bool,
    /// Номер режима скругления (0..=3).
    pub radius: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: false,
            radius: DEFAULT_RADIUS,
        }
    }
}

impl Settings {
    /// Радиус контролов в px для текущего режима.
    pub fn radius_px(self) -> f32 {
        radius_px(self.radius)
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
                    if let Some(mode) = value
                        .trim()
                        .parse::<i32>()
                        .ok()
                        .filter(|mode| (0..=3).contains(mode))
                    {
                        settings.radius = mode;
                    }
                }
                _ => {}
            }
        }
        settings
    }

    pub fn to_text(self) -> String {
        format!(
            "theme={}\nradius={}\n",
            if self.dark { "dark" } else { "light" },
            self.radius
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

/// Радиус контролов в px для режима (вне диапазона: обычное).
pub fn radius_px(mode: i32) -> f32 {
    usize::try_from(mode)
        .ok()
        .and_then(|i| RADIUS_PX.get(i).copied())
        .unwrap_or(RADIUS_PX[1])
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("ostov").join("ui.conf"))
}
