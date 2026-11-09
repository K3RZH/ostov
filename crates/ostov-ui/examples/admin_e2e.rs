//! Стенд экранов «Журнал действий» и «Закрытие периода»: настоящая демо-база, `AdminUi` и события
//! мыши, как их пришлёт система. Проверяет страницы по ключу, «Обновить», «Закрыть период»,
//! «Снять закрытие» и показ ошибки ядра.
//! Запуск: `env -u WAYLAND_DISPLAY SLINT_BACKEND=winit-software cargo run -p ostov-ui --example admin_e2e`.
// Стенд, а не приложение: падение на unwrap и есть провал сценария.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ostov_api::SystemRights;
use ostov_ui::admin::JOURNAL_PAGE;
use ostov_ui::admin_ui::AdminUi;
use ostov_ui::shell::Shell;
use ostov_ui::{AdminData, AppWindow};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model};

/// Крутит таймеры, пока условие не выполнилось (ответы ядра приходят через них).
fn wait_until(what: &str, condition: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        slint::platform::update_timers_and_animations();
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("  не дождались: {what}");
    false
}

fn settle() {
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(40));
        slint::platform::update_timers_and_animations();
    }
}

fn click(w: &AppWindow, x: f32, y: f32) {
    settle();
    let position = LogicalPosition::new(x, y);
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    w.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    settle();
}

fn open_tab(window: &AppWindow, shell: &Rc<RefCell<Shell>>, title: &str) {
    let node = shell
        .borrow()
        .nav_rows()
        .iter()
        .find(|row| row.title == title)
        .map(|row| row.id)
        .unwrap();
    shell.borrow_mut().click(node);
    ostov_ui::app::refresh(window, &shell.borrow());
    // Экран вкладки создаётся при отрисовке (там срабатывает его `init`), как в живом окне.
    window.window().take_snapshot().unwrap();
    settle();
}

fn main() {
    let path = std::env::temp_dir().join(format!("ostov-admin-e2e-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let engine = ostov_core::LocalConnector::default()
        .create_demo(path.to_str().unwrap(), 100)
        .unwrap();
    // Больше страницы журнала: тогда есть что дозагружать.
    let extra = usize::try_from(JOURNAL_PAGE).unwrap() + 10;
    for _ in 0..extra {
        engine.set_closing_date(Some("2026-01-31")).unwrap();
    }
    let total = engine.admin_journal(None, 1000).unwrap().len();

    let window = AppWindow::new().unwrap();
    window.set_page(2);
    window
        .window()
        .set_size(slint::LogicalSize::new(1440.0, 900.0));
    window.show().unwrap();
    let meta = ostov_api::Meta::new(&ostov_meta::demo().unwrap());
    let shell = Rc::new(RefCell::new(Shell::from_meta(&meta, SystemRights::ALL)));
    let receipts = ostov_ui::receipts_ui::setup(&window, shell.clone());
    let windows = ostov_ui::windows::Windows::new(&receipts);
    let admin = AdminUi::new(window.as_weak(), windows.clone(), {
        let engine = engine.clone();
        move || Some(engine.clone())
    });
    admin.attach_window(&window);
    windows.on_host_created({
        let admin = admin.clone();
        move |host| admin.attach_window(host)
    });
    admin.start(SystemRights::ALL);
    let data = window.global::<AdminData>();

    let mut failed = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "✓" } else { "✗" });
        if !ok {
            failed += 1;
        }
    };

    // Журнал: первая страница, «Ещё» есть.
    let page = usize::try_from(JOURNAL_PAGE).unwrap();
    let loaded = wait_until("первая страница", || {
        data.get_journal().row_count() == page
    });
    check("вход загружает первую страницу журнала", loaded);
    check("после полной страницы есть «Ещё»", data.get_journal_more());

    // Следующая страница дописывается и последняя: «Ещё» пропадает.
    data.invoke_journal_load_more();
    let all = wait_until("вторая страница", || {
        data.get_journal().row_count() == total
    });
    check("«Ещё» дописывает остальные записи", all);
    check(
        "после короткой страницы «Ещё» нет",
        !data.get_journal_more(),
    );

    // Новое действие видно после открытия вкладки: она перечитывает журнал сама.
    engine.set_closing_date(None).unwrap();
    open_tab(&window, &shell, "Журнал действий");
    let fresh = wait_until(
        "перечитывание при открытии вкладки",
        || data.get_journal().row_count() == page,
    );
    check(
        "открытие вкладки перечитывает журнал с первой страницы",
        fresh,
    );
    check(
        "новая запись сверху",
        data.get_journal()
            .row_data(0)
            .is_some_and(|row| row.details.contains("период открыт")),
    );

    // Кнопка «Обновить» (правый верхний угол вкладки) тоже возвращает первую страницу.
    data.invoke_journal_load_more();
    wait_until("вторая страница", || {
        data.get_journal().row_count() > page
    });
    click(&window, 1364.0, 130.0);
    let refreshed = wait_until("«Обновить»", || {
        data.get_journal().row_count() == page
    });
    check("«Обновить» возвращает первую страницу", refreshed);

    // Закрытие периода.
    open_tab(&window, &shell, "Закрытие периода");
    check(
        "вкладка читает дату закрытия: период открыт",
        wait_until("текст", || {
            data.get_closing_text() == "Период не закрыт"
        }),
    );
    data.set_closing_year(2026);
    data.set_closing_month(10);
    data.set_closing_day(31);
    // Кнопки по снимку экрана: «Закрыть период» (550, 285), «Снять закрытие» (714, 285), а под
    // плашкой ошибки на 60 px ниже.
    click(&window, 550.0, 285.0);
    let closed = wait_until("закрытие", || data.get_closing_set());
    check("«Закрыть период» закрывает период в ядре", closed);
    check(
        "ядро хранит выбранную дату",
        engine.closing_date().unwrap().as_deref() == Some("2026-10-31"),
    );
    check(
        "на экране дата закрытия",
        data.get_closing_text() == "Период закрыт по 31.10.2026",
    );

    // Несуществующий день ядро отклоняет, его слова показываются как есть.
    data.set_closing_month(2);
    click(&window, 550.0, 285.0);
    let refused = wait_until("ошибка", || !data.get_closing_error().is_empty());
    check(
        "ошибка ядра показана как есть",
        refused && data.get_closing_error().contains("не дата"),
    );
    check(
        "закрытие осталось прежним",
        engine.closing_date().unwrap().as_deref() == Some("2026-10-31"),
    );

    click(&window, 714.0, 345.0);
    let opened = wait_until("снятие закрытия", || !data.get_closing_set());
    check("«Снять закрытие» открывает период", opened);
    check(
        "в ядре периода больше нет закрытия",
        engine.closing_date().unwrap().is_none(),
    );
    check(
        "ошибка пропала после удачной команды",
        data.get_closing_error().is_empty(),
    );

    // Вкладка в отдельном окне: у окна свой набор данных, тот же `AdminUi` ведёт их обоих.
    open_tab(&window, &shell, "Журнал действий");
    let tab = shell.borrow().active_tab();
    windows.detach_from_main(&window, &shell, tab);
    let host = windows
        .debug_state()
        .0
        .first()
        .and_then(|id| windows.host(*id))
        .unwrap();
    host.window().take_snapshot().unwrap();
    settle();
    let host_data = host.global::<AdminData>();
    check(
        "дополнительное окно получает журнал",
        wait_until("журнал в окне", || {
            host_data.get_journal().row_count() == page
        }),
    );
    engine.set_closing_date(Some("2026-11-30")).unwrap();
    host_data.invoke_journal_refresh();
    check(
        "«Обновить» в дополнительном окне обновляет оба окна",
        wait_until("запись в обоих окнах", || {
            let has = |data: &AdminData| {
                data.get_journal()
                    .row_data(0)
                    .is_some_and(|row| row.details.contains("30.11.2026"))
            };
            has(&host_data) && has(&data)
        }),
    );

    if failed > 0 {
        println!("провалено: {failed}");
        std::process::exit(1);
    }
    // База и её служебные файлы (замок, журнал SQLite) во временной папке.
    for suffix in ["", ".lock", "-shm", "-wal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
