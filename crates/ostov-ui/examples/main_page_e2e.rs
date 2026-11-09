//! Стенд главной страницы: окну посылаются настоящие события мыши и клавиатуры, как их пришлёт
//! система. Проверяет «Пользователи и роли» (кнопки кликаются, окно «Новый пользователь»
//! принимает ввод, «Добавить» срабатывает) и меню под аватаром в шапке.
//! Запуск: `env -u WAYLAND_DISPLAY SLINT_BACKEND=winit-software cargo run -p ostov-ui --example main_page_e2e`.
// Стенд, а не приложение: падение на unwrap и есть провал сценария.
#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::rc::Rc;

use ostov_ui::AppWindow;
use ostov_ui::app::{refresh, refresh_users};
use ostov_ui::shell::Shell;
use ostov_ui::users::Directory;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, SharedString, VecModel};

fn settle() {
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(40));
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

fn type_text(w: &AppWindow, text: &str) {
    for ch in text.chars() {
        let text = SharedString::from(ch.to_string());
        w.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        w.window().dispatch_event(WindowEvent::KeyReleased { text });
    }
    settle();
}

fn main() {
    let window = AppWindow::new().unwrap();
    window.set_page(2);
    window
        .window()
        .set_size(slint::LogicalSize::new(1440.0, 900.0));
    window.show().unwrap();

    let meta = ostov_api::Meta::new(&ostov_meta::demo().unwrap());
    let mut shell = Shell::from_meta(&meta, ostov_api::SystemRights::ALL);
    let users_node = shell
        .nav_rows()
        .iter()
        .find(|row| row.title == "Пользователи и роли")
        .map(|row| row.id)
        .unwrap();
    shell.click(users_node);
    refresh(&window, &shell);
    refresh_users(&window, &Directory::demo());
    window.set_roles(ModelRc::new(VecModel::from(
        ["admin", "storekeeper"].map(SharedString::from).to_vec(),
    )));

    let confirmed = Rc::new(Cell::new(0));
    let toggled = Rc::new(Cell::new(-1));
    window.on_user_add_confirmed({
        let confirmed = confirmed.clone();
        move || confirmed.set(confirmed.get() + 1)
    });
    window.on_user_toggle({
        let toggled = toggled.clone();
        move |row| toggled.set(row)
    });
    let password_row = Rc::new(Cell::new(-1));
    window.on_user_password({
        let password_row = password_row.clone();
        move |row| password_row.set(row)
    });
    let closed = Rc::new(Cell::new(0));
    let switched = Rc::new(Cell::new(0));
    window.on_close_base({
        let closed = closed.clone();
        move || closed.set(closed.get() + 1)
    });
    window.on_switch_user({
        let switched = switched.clone();
        move || switched.set(switched.get() + 1)
    });
    window.set_user_text("Админ".into());
    window.set_user_initials("А".into());
    window.set_user_roles_text("admin".into());
    window.set_can_switch_user(false);
    settle();

    let mut failed = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "✓" } else { "✗" });
        if !ok {
            failed += 1;
        }
    };

    click(&window, 1314.0, 130.0);
    check(
        "«Добавить пользователя» открывает окно",
        window.get_adding_user(),
    );

    click(&window, 720.0, 309.0);
    type_text(&window, "admin");
    check(
        "логин вводится с клавиатуры",
        window.get_nu_login() == "admin",
    );

    click(&window, 720.0, 382.0);
    type_text(&window, "Админ");
    check("имя вводится", window.get_nu_name() == "Админ");

    click(&window, 860.0, 459.0);
    check("роль выбирается кликом", window.get_nu_role() == 1);
    click(&window, 560.0, 459.0);
    check("роль меняется обратно", window.get_nu_role() == 0);

    click(&window, 720.0, 532.0);
    type_text(&window, "длинный-пароль-1");
    click(&window, 720.0, 605.0);
    type_text(&window, "длинный-пароль-1");
    check(
        "пароль и повтор введены",
        window.get_nu_password() == "длинный-пароль-1"
            && window.get_nu_repeat() == "длинный-пароль-1",
    );

    click(&window, 965.0, 657.0);
    check("«Добавить» срабатывает", confirmed.get() == 1);

    window.set_adding_user(false);
    settle();
    click(&window, 1395.0, 238.0);
    check("«Отключить» в строке срабатывает", toggled.get() == 0);
    click(&window, 1260.0, 282.0);
    check(
        "«Сменить пароль» во второй строке срабатывает",
        password_row.get() == 1,
    );

    // Меню под аватаром: пункт «Сменить пользователя» недоступен, пока база без входа.
    click(&window, 1276.0, 24.0);
    click(&window, 1312.0, 126.0);
    check(
        "«Сменить пользователя» недоступен без входа",
        switched.get() == 0,
    );
    click(&window, 1312.0, 158.0);
    check(
        "«Закрыть базу» в меню аватара срабатывает",
        closed.get() == 1,
    );

    window.set_can_switch_user(true);
    click(&window, 1276.0, 24.0);
    click(&window, 1312.0, 126.0);
    check(
        "«Сменить пользователя» срабатывает при входе",
        switched.get() == 1,
    );

    if failed > 0 {
        println!("провалено: {failed}");
        std::process::exit(1);
    }
}
