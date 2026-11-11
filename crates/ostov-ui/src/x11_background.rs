//! Фон X11-окна цветом темы, чтобы при растягивании не мелькала чёрная область.
//!
//! Winit создаёт X-окно без фона. Когда окно растёт, X-сервер сразу открывает новую полосу
//! справа и снизу, а приложение перерисует её позже; до этого её содержимое не определено и
//! выглядит чёрным. Если задать окну `background_pixel` цветом темы, сервер закрасит полосу сам,
//! а `bit_gravity = NorthWest` оставит уже нарисованное на месте, не стирая его на каждом шаге.
//!
//! На Wayland и вне Linux модуль ничего не делает. Синхронизации ресайза с композитором
//! (`_NET_WM_SYNC_REQUEST`) в winit 0.30 нет, она здесь не чинится.

pub use imp::{Watcher, set_theme_color};

#[cfg(target_os = "linux")]
mod imp {
    use std::cell::Cell;
    use std::fmt;
    use std::sync::atomic::{AtomicBool, Ordering};

    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection;
    use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, Gravity, VisualClass};

    type Rgb = [u8; 3];

    const RED_MASK: u32 = 0x00FF_0000;
    const GREEN_MASK: u32 = 0x0000_FF00;
    const BLUE_MASK: u32 = 0x0000_00FF;
    /// В 32-битном визуале старший байт это альфа; фон должен быть непрозрачным.
    const OPAQUE_ALPHA: u32 = 0xFF00_0000;

    thread_local! {
        /// Фон окон по теме. Тема одна на всё приложение, интерфейс живёт в одном потоке.
        static THEME_COLOR: Cell<Option<Rgb>> = const { Cell::new(None) };
    }

    /// Одно сообщение об ошибке на процесс: события приходят постоянно, спамить нельзя.
    static REPORTED: AtomicBool = AtomicBool::new(false);

    /// Запоминает цвет фона (`Theme.bg`) и красит им окно, если его X-окно уже создано.
    /// Окна, которых ещё нет, покрасит [`Watcher`] при первом событии.
    pub fn set_theme_color(window: &slint::Window, color: slint::Color) {
        let rgb = [color.red(), color.green(), color.blue()];
        THEME_COLOR.set(Some(rgb));
        if let Some(xid) = x11_window(window) {
            report(paint(xid, rgb));
        }
    }

    /// Следит за окном: X-окно у Slint появляется только после показа и может быть создано
    /// заново, поэтому красим его, когда виден новый идентификатор.
    #[derive(Default)]
    pub struct Watcher {
        painted: Cell<Option<u32>>,
    }

    impl Watcher {
        pub fn new() -> Self {
            Self::default()
        }

        /// Вызывается на каждое событие окна. Для уже окрашенного окна это пара сравнений.
        pub fn on_event(&self, window: &slint::Window) {
            let Some(rgb) = THEME_COLOR.get() else { return };
            let Some(xid) = x11_window(window) else {
                return;
            };
            // Идентификатор запоминаем и при ошибке: повторять попытку на каждое событие нельзя.
            if self.painted.replace(Some(xid)) != Some(xid) {
                report(paint(xid, rgb));
            }
        }
    }

    /// Идентификатор X-окна или `None`: окно ещё не создано либо идёт не через X11 (Wayland).
    fn x11_window(window: &slint::Window) -> Option<u32> {
        window
            .with_winit_window(|winit| match winit.window_handle().ok()?.as_raw() {
                RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
                RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
                _ => None,
            })
            .flatten()
    }

    /// Ставит окну `xid` фон и гравитацию. Визуал не TrueColor 24/32 бит: ничего не меняет.
    fn paint(xid: u32, rgb: Rgb) -> Result<(), PaintError> {
        let (connection, _) = x11rb::connect(None)?;
        let visual_id = connection.get_window_attributes(xid)?.reply()?.visual;
        // Глубина окна определяется его визуалом, поэтому ищем визуал среди всех экранов.
        let visual = connection
            .setup()
            .roots
            .iter()
            .flat_map(|screen| &screen.allowed_depths)
            .find_map(|depth| {
                let visual = depth.visuals.iter().find(|v| v.visual_id == visual_id)?;
                Some((depth.depth, visual))
            });
        let Some((depth, visual)) = visual else {
            return Ok(());
        };
        if visual.class != VisualClass::TRUE_COLOR {
            return Ok(());
        }
        let masks = [visual.red_mask, visual.green_mask, visual.blue_mask];
        let Some(pixel) = pixel_for(rgb, depth, masks) else {
            return Ok(());
        };
        let attributes = ChangeWindowAttributesAux::new()
            .background_pixel(pixel)
            .bit_gravity(Gravity::NORTH_WEST);
        // `check` ждёт ответа сервера: ошибку (окно уже закрыто) видно сразу, а не после закрытия связи.
        connection
            .change_window_attributes(xid, &attributes)?
            .check()?;
        Ok(())
    }

    /// Значение пикселя для цвета в визуале `depth` бит с масками каналов `masks` (R, G, B).
    /// Только привычная раскладка 0xRRGGBB; другую не угадываем и фон не трогаем.
    fn pixel_for([red, green, blue]: Rgb, depth: u8, masks: [u32; 3]) -> Option<u32> {
        if masks != [RED_MASK, GREEN_MASK, BLUE_MASK] {
            return None;
        }
        let rgb = u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue);
        match depth {
            24 => Some(rgb),
            32 => Some(OPAQUE_ALPHA | rgb),
            _ => None,
        }
    }

    fn report(result: Result<(), PaintError>) {
        if let Err(error) = result
            && !REPORTED.swap(true, Ordering::Relaxed)
        {
            eprintln!(
                "Ostov: не удалось задать фон X11-окна, при растягивании возможно мерцание: {error}"
            );
        }
    }

    #[derive(Debug)]
    enum PaintError {
        Connect(ConnectError),
        Request(ReplyError),
    }

    impl fmt::Display for PaintError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Connect(error) => write!(f, "нет связи с X-сервером: {error}"),
                Self::Request(error) => write!(f, "запрос к X-серверу не прошёл: {error}"),
            }
        }
    }

    impl From<ConnectError> for PaintError {
        fn from(error: ConnectError) -> Self {
            Self::Connect(error)
        }
    }

    impl From<ConnectionError> for PaintError {
        fn from(error: ConnectionError) -> Self {
            Self::Request(error.into())
        }
    }

    impl From<ReplyError> for PaintError {
        fn from(error: ReplyError) -> Self {
            Self::Request(error)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const STANDARD_MASKS: [u32; 3] = [RED_MASK, GREEN_MASK, BLUE_MASK];

        #[test]
        fn depth_24_gives_plain_rrggbb() {
            // Тёмная тема: Theme.bg = #1F1512.
            assert_eq!(
                pixel_for([0x1F, 0x15, 0x12], 24, STANDARD_MASKS),
                Some(0x001F_1512)
            );
        }

        #[test]
        fn depth_32_adds_opaque_alpha() {
            // Светлая тема: Theme.bg = #FAF6EE.
            assert_eq!(
                pixel_for([0xFA, 0xF6, 0xEE], 32, STANDARD_MASKS),
                Some(0xFFFA_F6EE)
            );
        }

        #[test]
        fn unusual_depth_is_left_alone() {
            assert_eq!(pixel_for([1, 2, 3], 16, STANDARD_MASKS), None);
            assert_eq!(pixel_for([1, 2, 3], 30, STANDARD_MASKS), None);
        }

        #[test]
        fn unusual_channel_layout_is_left_alone() {
            let bgr = [BLUE_MASK, GREEN_MASK, RED_MASK];
            assert_eq!(pixel_for([1, 2, 3], 24, bgr), None);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    /// Заглушка: на этой системе фон окна задаёт сама оболочка.
    pub fn set_theme_color(_window: &slint::Window, _color: slint::Color) {}

    #[derive(Default)]
    pub struct Watcher;

    impl Watcher {
        pub fn new() -> Self {
            Self
        }

        pub fn on_event(&self, _window: &slint::Window) {}
    }
}
