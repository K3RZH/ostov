//! Сквозной стенд перетаскивания вкладок: окну посылаются настоящие события мыши
//! (нажатие, движение, отпускание), как их пришлёт система, и после каждого сценария
//! проверяется, что окна по модели и на экране совпадают, а вкладки не потерялись.
//! Запуск: `env -u WAYLAND_DISPLAY SLINT_BACKEND=winit-software cargo run -p ostov-ui --example drag_e2e`.
// Стенд, а не приложение: падение на unwrap и есть провал сценария.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::rc::Rc;

use ostov_ui::AppWindow;
use ostov_ui::shell::Shell;
use ostov_ui::windows::Windows;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};

struct Rig {
    main: AppWindow,
    shell: Rc<RefCell<Shell>>,
    windows: Windows,
    failed: u32,
}

fn pos(x: f32, y: f32) -> LogicalPosition {
    LogicalPosition::new(x, y)
}

fn press(w: &AppWindow, x: f32, y: f32) {
    settle();
    w.window().dispatch_event(WindowEvent::PointerPressed {
        position: pos(x, y),
        button: PointerEventButton::Left,
    });
}

fn mv(w: &AppWindow, x: f32, y: f32) {
    w.window().dispatch_event(WindowEvent::PointerMoved {
        position: pos(x, y),
    });
}

fn release(w: &AppWindow, x: f32, y: f32) {
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position: pos(x, y),
        button: PointerEventButton::Left,
    });
}

/// Даёт анимациям и таймерам дойти до конца: без этого вкладки после перестановки ещё «едут»,
/// и следующее нажатие промахивается.
fn settle() {
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(60));
        slint::platform::update_timers_and_animations();
    }
}

/// Положение указателя на экране (физические пиксели) по точке внутри окна.
fn global_of(w: &AppWindow, local: (f32, f32)) -> (f32, f32) {
    let p = w.window().position();
    let s = w.window().scale_factor();
    (p.x as f32 + local.0 * s, p.y as f32 + local.1 * s)
}

/// Событие указателя в окне для точки экрана: локальные координаты считаются по ТЕКУЩЕМУ
/// положению окна, как их присылает система (окно, которое идёт за курсором, уже сдвинулось).
fn local_of(w: &AppWindow, global: (f32, f32)) -> (f32, f32) {
    let p = w.window().position();
    let s = w.window().scale_factor();
    ((global.0 - p.x as f32) / s, (global.1 - p.y as f32) / s)
}

/// Ведёт указатель по экрану от точки `from` к `to` шагами: окно-источник получает события.
fn drag_global(w: &AppWindow, from: (f32, f32), to: (f32, f32), steps: u32) {
    for step in 1..=steps {
        let k = step as f32 / steps as f32;
        let g = (from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k);
        let l = local_of(w, g);
        mv(w, l.0, l.1);
    }
}

/// Центр вкладки `i` в окне (логические пиксели окна).
fn tab_center(w: &AppWindow, i: usize) -> (f32, f32) {
    let tw = w.get_strip_tab_width();
    (
        w.get_strip_left() + 8.0 + i as f32 * tw + tw / 2.0,
        w.get_strip_top() + 20.0,
    )
}

/// Точка в окне `target`, которая в общих координатах совпадает с точкой `local` окна `source`.
fn to_other(source: &AppWindow, target: &AppWindow, local: (f32, f32)) -> (f32, f32) {
    let (ps, pt) = (source.window().position(), target.window().position());
    let s = source.window().scale_factor();
    (
        (ps.x as f32 + local.0 * s - pt.x as f32) / s,
        (ps.y as f32 + local.1 * s - pt.y as f32) / s,
    )
}

impl Rig {
    fn new() -> Self {
        let main = AppWindow::new().unwrap();
        main.set_page(2);
        main.window()
            .set_size(slint::LogicalSize::new(1300.0, 800.0));
        main.show().unwrap();
        let shell = Rc::new(RefCell::new(Shell::demo()));
        for node in [2, 4, 6, 7, 15, 3] {
            let mut shell = shell.borrow_mut();
            shell.click(node);
            let active = shell.active_tab();
            shell.pin_tab(active);
        }
        let receipts = ostov_ui::receipts_ui::setup(&main, shell.clone());
        let windows = Windows::new(&receipts);
        ostov_ui::app::wire_tabs(&main, &shell, &receipts, &windows);
        ostov_ui::app::refresh(&main, &shell.borrow());
        Self {
            main,
            shell,
            windows,
            failed: 0,
        }
    }

    fn main_tabs(&self) -> usize {
        self.shell.borrow().tabs().len()
    }

    /// Первая вкладка, которую можно вынести в окно.
    fn movable(&self) -> usize {
        self.shell
            .borrow()
            .tabs()
            .iter()
            .position(|t| t.view != "receipts")
            .unwrap_or(0)
    }

    fn list_tab(&self) -> Option<usize> {
        self.shell
            .borrow()
            .tabs()
            .iter()
            .position(|t| t.view == "receipts")
    }

    fn total_tabs(&self) -> usize {
        self.shell.borrow().tabs().len() + self.windows.debug_state().2.iter().sum::<usize>()
    }

    fn check(&mut self, name: &str, cond: bool, detail: String) {
        if cond {
            println!("  ✓ {name}");
        } else {
            self.failed += 1;
            println!("  ✗ {name}: {detail}");
        }
    }

    /// Окна по модели и на экране совпадают, вкладок не потеряно.
    fn consistent(&mut self, label: &str, expected_total: usize) {
        let (model, screen, _) = self.windows.debug_state();
        let total = self.total_tabs();
        self.check(
            &format!("{label}: окна модели и экрана совпадают"),
            model == screen,
            format!("модель {model:?}, экран {screen:?}"),
        );
        self.check(
            &format!("{label}: вкладки не потеряны"),
            total == expected_total,
            format!("{total} вместо {expected_total}"),
        );
    }
}

fn main() {
    let mut rig = Rig::new();
    let total = rig.total_tabs();
    let kinds: Vec<(String, &str)> = rig
        .shell
        .borrow()
        .tabs()
        .iter()
        .map(|t| (t.title.clone(), t.view))
        .collect();
    println!(
        "старт: вкладок {total}, панель left={:.0} top={:.0} ширина вкладки={:.0}",
        rig.main.get_strip_left(),
        rig.main.get_strip_top(),
        rig.main.get_strip_tab_width()
    );
    println!("вкладки: {kinds:?}");
    // Вкладка без состояния, которую можно выносить.
    println!("\nсценарий 1: порядок внутри панели");
    {
        let before: Vec<String> = kinds.iter().map(|k| k.0.clone()).collect();
        let (x, y) = tab_center(&rig.main, 0);
        press(&rig.main, x, y);
        for step in 1..=12 {
            mv(&rig.main, x + step as f32 * 14.0, y);
        }
        release(&rig.main, x + 12.0 * 14.0, y);
        let after: Vec<String> = rig
            .shell
            .borrow()
            .tabs()
            .iter()
            .map(|t| t.title.clone())
            .collect();
        rig.check("порядок изменился", before != after, format!("{after:?}"));
        rig.consistent("после перестановки", total);
        assert!(rig.windows.debug_state().0.is_empty());
    }

    println!("\nсценарий 2: вынос вкладки и отпускание вне панелей");
    {
        let movable = rig.movable();
        let (x, y) = tab_center(&rig.main, movable);
        press(&rig.main, x, y);
        for step in 1..=20 {
            mv(&rig.main, x + step as f32 * 3.0, y + step as f32 * 6.0);
        }
        let mid = rig.windows.debug_state();
        rig.check(
            "во время перетаскивания окно-копия есть",
            mid.0.len() == 1,
            format!("{mid:?}"),
        );
        let now = rig.main_tabs();
        rig.check(
            "во время перетаскивания вкладка ещё в главном окне",
            now == total,
            format!("{now}"),
        );
        release(&rig.main, x + 60.0, y + 120.0);
        let after = rig.windows.debug_state();
        rig.check(
            "после отпускания одно окно",
            after.0.len() == 1,
            format!("{after:?}"),
        );
        let now = rig.main_tabs();
        rig.check(
            "вкладка ушла из главного окна",
            now == total - 1,
            format!("{now}"),
        );
        rig.consistent("после вноса", total);
    }

    println!("\nсценарий 3: вынос и возврат в свою панель (отмена)");
    {
        let before = rig.windows.debug_state();
        let main_tabs = rig.shell.borrow().tabs().len();
        let (x, y) = tab_center(&rig.main, 0);
        press(&rig.main, x, y);
        for step in 1..=12 {
            mv(&rig.main, x + step as f32 * 3.0, y + step as f32 * 7.0);
        }
        for step in (0..=12).rev() {
            mv(&rig.main, x + step as f32 * 3.0, y + step as f32 * 7.0);
        }
        release(&rig.main, x, y);
        let after = rig.windows.debug_state();
        rig.check(
            "число окон не изменилось",
            before.0.len() == after.0.len(),
            format!("{before:?} -> {after:?}"),
        );
        let now = rig.main_tabs();
        rig.check(
            "вкладки главного окна те же",
            now == main_tabs,
            format!("{now}"),
        );
        rig.consistent("после отмены", total);
    }

    println!("\nсценарий 4: бросок вкладки главного окна на панель дополнительного");
    {
        let id = rig.windows.debug_state().0[0];
        let host = rig.windows.host(id).unwrap();
        let target = tab_center(&host, 0);
        let (x, y) = tab_center(&rig.main, 0);
        let drop_at = to_other(&host, &rig.main, (target.0 + 30.0, target.1));
        let host_tabs_before = rig.windows.debug_state().2[0];
        press(&rig.main, x, y);
        for step in 1..=10 {
            mv(&rig.main, x + step as f32 * 2.0, y + step as f32 * 8.0);
        }
        let n = 15;
        for step in 1..=n {
            let k = step as f32 / n as f32;
            mv(
                &rig.main,
                x + 20.0 + (drop_at.0 - x - 20.0) * k,
                y + 80.0 + (drop_at.1 - y - 80.0) * k,
            );
        }
        release(&rig.main, drop_at.0, drop_at.1);
        let st = rig.windows.debug_state();
        rig.check(
            "в дополнительном окне стало на вкладку больше",
            st.2.first().copied() == Some(host_tabs_before + 1),
            format!("{st:?}, было {host_tabs_before}"),
        );
        rig.consistent("после броска на панель", total);
    }

    println!("\nсценарий 5: окно с одной вкладкой тащится целиком");
    {
        let mut rig = Rig::new();
        let total = rig.total_tabs();
        // Получаем окно с одной вкладкой.
        let movable = rig.movable();
        let (x, y) = tab_center(&rig.main, movable);
        press(&rig.main, x, y);
        for step in 1..=20 {
            mv(&rig.main, x + step as f32 * 3.0, y + step as f32 * 6.0);
        }
        release(&rig.main, x + 60.0, y + 120.0);
        let st = rig.windows.debug_state();
        rig.check("есть окно с одной вкладкой", st.2 == [1], format!("{st:?}"));
        let id = st.0[0];
        let host = rig.windows.host(id).unwrap();

        // 5а. Тащим окно и отпускаем на пустом месте: окно остаётся.
        let start = tab_center(&host, 0);
        let g0 = global_of(&host, start);
        press(&host, start.0, start.1);
        let g1 = (g0.0 + 300.0, g0.1 + 250.0);
        drag_global(&host, g0, g1, 15);
        let l = local_of(&host, g1);
        release(&host, l.0, l.1);
        let st = rig.windows.debug_state();
        rig.check(
            "после броска на пустое место окно живо",
            st.0 == [id],
            format!("{st:?}"),
        );
        rig.consistent("после переноса окна", total);

        // 5б. Тащим окно на панель главного окна: окно сливается.
        let start = tab_center(&host, 0);
        let g0 = global_of(&host, start);
        let main_strip = global_of(&rig.main, tab_center(&rig.main, 0));
        press(&host, start.0, start.1);
        drag_global(&host, g0, (g0.0, g0.1 + 40.0), 4);
        drag_global(&host, (g0.0, g0.1 + 40.0), main_strip, 20);
        let l = local_of(&host, main_strip);
        release(&host, l.0, l.1);
        let st = rig.windows.debug_state();
        rig.check("окно слилось с главным", st.0.is_empty(), format!("{st:?}"));
        let main_now = rig.main_tabs();
        rig.check(
            "вкладка вернулась в главное окно",
            main_now == total,
            format!("{main_now}"),
        );
        rig.consistent("после слияния окна", total);
        if rig.failed > 0 {
            std::process::exit(1);
        }
    }

    println!("\nсценарий 6: вкладка со списком выносится, как любая другая");
    {
        if let Some(i) = rig.list_tab() {
            let before_main = rig.shell.borrow().tabs().len();
            let before_win = rig.windows.debug_state().0.len();
            let (x, y) = tab_center(&rig.main, i);
            press(&rig.main, x, y);
            for step in 1..=20 {
                mv(&rig.main, x + step as f32 * 3.0, y + step as f32 * 6.0);
            }
            release(&rig.main, x + 60.0, y + 120.0);
            let st = rig.windows.debug_state();
            println!(
                "    окон {} -> {}, вкладок в главном {} -> {}",
                before_win,
                st.0.len(),
                before_main,
                rig.shell.borrow().tabs().len()
            );
            let now = rig.shell.borrow().tabs().len();
            rig.check(
                "список ушёл из главного окна",
                now + 1 == before_main,
                format!("{now}"),
            );
            rig.consistent("после вытаскивания списка", total);
        } else {
            println!("  (вкладки со списком нет)");
        }
    }

    println!("\nитог: {} провалов", rig.failed);
    std::process::exit(i32::from(rig.failed > 0));
}
