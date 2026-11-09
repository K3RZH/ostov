//! Стенд страницы выбора базы: окну посылаются настоящие события мыши, как их пришлёт система.
//! Проверяет «Убрать из списка»: крестик у выбранной строки, подтверждение, отмену, выбор
//! соседней строки, забывание логина и сертификата, сохранение файлов и ошибку записи списка.
//! Файлы настроек стенд пишет в свою временную папку: для этого запускает сам себя с
//! `XDG_CONFIG_HOME` (в коде приложения менять переменные окружения нельзя, `unsafe` запрещён).
//! Запуск: `env -u WAYLAND_DISPLAY SLINT_BACKEND=winit-software cargo run -p ostov-ui --example start_e2e`.
// Стенд, а не приложение: падение на unwrap и есть провал сценария.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use ostov_ui::app::wire_forget;
use ostov_ui::recent::{remember_login, remembered_login};
use ostov_ui::{AppWindow, RecentDb, recent, remote_base};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, ModelRc, VecModel};

/// Признак, что это уже дочерний запуск со своей папкой настроек.
const CHILD_MARK: &str = "OSTOV_START_E2E_CHILD";
const SERVER: &str = "server.local:7700";
const PEM: &str = "-----BEGIN CERTIFICATE-----\nabc\n-----END CERTIFICATE-----\n";

/// Координаты со снимка `snapshot ... start`: окно 1200x760, строки через 56 пикселей. Колонка
/// списка стоит по центру окна, поэтому с каждой убранной строкой всё сдвигается вниз на 28.
const ROW_X: f32 = 800.0;
const CROSS_X: f32 = 1049.0;
fn row_y(count: u8, index: u8) -> f32 {
    322.0 + 28.0 * f32::from(3 - count) + 56.0 * f32::from(index)
}
/// Кнопки подтверждения: карточка по центру, её высота зависит от длины текста, поэтому
/// для файла и для сервера кнопки на разной высоте.
const CANCEL: (f32, f32) = (648.0, 476.0);
const ACCEPT_FILE: (f32, f32) = (745.0, 476.0);
const ACCEPT_SERVER: (f32, f32) = (745.0, 467.0);
/// «Добавить существующую…» за краем карточки: под подтверждением она не должна срабатывать.
const ADD_BUTTON: (f32, f32) = (960.0, 494.0);

fn settle() {
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(40));
        slint::platform::update_timers_and_animations();
    }
}

fn click(w: &AppWindow, (x, y): (f32, f32)) {
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

fn names(w: &AppWindow) -> Vec<String> {
    w.get_recent()
        .iter()
        .map(|db| db.name.to_string())
        .collect()
}

fn saved_names() -> Vec<String> {
    recent::load()
        .iter()
        .map(|db| db.name.to_string())
        .collect()
}

fn row(name: &str, path: &Path, server: bool) -> RecentDb {
    RecentDb {
        name: name.into(),
        path: path.to_string_lossy().as_ref().into(),
        when: "".into(),
        server,
        protection: 0,
    }
}

/// Родитель: готовит папку настроек, запускает стенд в ней и отдаёт его код возврата.
fn run_in_temp_config() -> ! {
    let dir = std::env::temp_dir().join(format!("ostov-start-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .env("XDG_CONFIG_HOME", &dir)
        .env(CHILD_MARK, "1")
        .status()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    std::process::exit(status.code().unwrap_or(1));
}

/// Три базы (файл, сервер, файл), по логину у каждой, сертификат сервера и чужой сертификат.
/// Файлы баз настоящие (пути относительно рабочей папки): после «Убрать» они должны остаться.
/// Пути короткие: длинный путь расширил бы колонку списка, и координаты со снимка сдвинулись бы.
fn seed(w: &AppWindow) -> Vec<RecentDb> {
    let rows = vec![
        row("Склад.sqlite", Path::new("Склад.sqlite"), false),
        row("Кафе", Path::new(SERVER), true),
        row("Бар.sqlite", Path::new("Бар.sqlite"), false),
    ];
    for db in &rows {
        remember_login(&db.path, Some("ann"));
        if !db.server {
            std::fs::write(db.path.as_str(), "база").unwrap();
        }
    }
    let _ = std::fs::remove_dir_all(config_dir().join("recent.conf"));
    recent::save(&rows).unwrap();
    remote_base::store_certificate(SERVER, PEM).unwrap();
    remote_base::store_certificate("other:1", PEM).unwrap();
    w.set_recent(ModelRc::new(VecModel::from(rows.clone())));
    w.set_selected_recent(0);
    w.set_start_error("".into());
    w.set_start_busy(false);
    w.set_forgetting(false);
    rows
}

fn config_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()).join("ostov")
}

fn main() {
    if std::env::var_os(CHILD_MARK).is_none() {
        run_in_temp_config();
    }
    let bases = config_dir().join("bases");
    std::fs::create_dir_all(&bases).unwrap();
    std::env::set_current_dir(&bases).unwrap();

    let window = AppWindow::new().unwrap();
    window.set_page(0);
    window
        .window()
        .set_size(slint::LogicalSize::new(1200.0, 760.0));
    window.show().unwrap();
    wire_forget(&window);

    let mut failed = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "✓" } else { "✗" });
        if !ok {
            failed += 1;
        }
    };

    let rows = seed(&window);
    let (file0, file2) = (rows[0].path.to_string(), rows[2].path.to_string());
    settle();

    // Крестик виден у строки под мышкой: щелчок по нему выбирает эту строку и спрашивает о ней.
    click(&window, (CROSS_X, row_y(3, 1)));
    check(
        "крестик строки под мышкой выбирает её и открывает подтверждение",
        window.get_selected_recent() == 1 && window.get_forgetting(),
    );
    click(&window, CANCEL);
    check("отмена закрывает подтверждение", !window.get_forgetting());
    click(&window, (ROW_X, row_y(3, 0)));
    check("выбрана первая строка", window.get_selected_recent() == 0);

    // Отмена ничего не меняет.
    click(&window, (CROSS_X, row_y(3, 0)));
    check("крестик открывает подтверждение", window.get_forgetting());
    click(&window, ADD_BUTTON);
    check(
        "пока открыто подтверждение, кнопки под ним не срабатывают",
        !window.get_adding() && window.get_forgetting(),
    );
    click(&window, CANCEL);
    check(
        "«Отмена» закрывает окно и ничего не убирает",
        !window.get_forgetting()
            && names(&window).len() == 3
            && saved_names().len() == 3
            && remembered_login(&file0).is_some(),
    );

    // Строка с файлом (первая): запись, логин, файл базы.
    click(&window, (CROSS_X, row_y(3, 0)));
    click(&window, ACCEPT_FILE);
    check(
        "«Убрать» убирает выбранную строку из списка",
        names(&window) == ["Кафе", "Бар.sqlite"] && !window.get_forgetting(),
    );
    check(
        "список в recent.conf переписан",
        saved_names() == ["Кафе", "Бар.sqlite"],
    );
    check(
        "запомненный логин убранной базы забыт, остальные остались",
        remembered_login(&file0).is_none()
            && remembered_login(SERVER).is_some()
            && remembered_login(&file2).is_some(),
    );
    check(
        "файл базы на диске остался",
        Path::new(&file0).exists() && Path::new(&file2).exists(),
    );
    check(
        "сертификат сервера не тронут, пока сервер в списке",
        remote_base::stored_certificate(SERVER).is_ok(),
    );
    check(
        "выбрана строка, вставшая на место",
        window.get_selected_recent() == 0,
    );

    // Последняя строка: выбор переходит на предыдущую.
    click(&window, (ROW_X, row_y(2, 1)));
    click(&window, (CROSS_X, row_y(2, 1)));
    click(&window, ACCEPT_FILE);
    check(
        "последняя строка убрана",
        names(&window) == ["Кафе"] && saved_names() == ["Кафе"],
    );
    check(
        "выбор ушёл на предыдущую строку",
        window.get_selected_recent() == 0,
    );

    // Сервер: ещё и сертификат; список становится пустым.
    click(&window, (CROSS_X, row_y(1, 0)));
    click(&window, ACCEPT_SERVER);
    check(
        "сервер убран, список пуст",
        names(&window).is_empty() && saved_names().is_empty(),
    );
    check(
        "сертификат этого сервера удалён, чужой остался",
        remote_base::stored_certificate(SERVER).is_err()
            && remote_base::stored_certificate("other:1").is_ok(),
    );
    check("логин сервера забыт", remembered_login(SERVER).is_none());

    // База открывается: убрать нельзя ни крестиком, ни прямым вызовом.
    let rows = seed(&window);
    window.set_start_busy(true);
    click(&window, (CROSS_X, row_y(3, 0)));
    check(
        "при открытии базы крестик не работает",
        !window.get_forgetting(),
    );
    window.invoke_forget_confirmed(0);
    check(
        "при открытии базы список не меняется и прямым вызовом",
        names(&window).len() == 3 && saved_names().len() == 3,
    );
    window.set_start_busy(false);

    // Список не записался: в окне остаётся то, что на диске, и видна ошибка.
    let conf = config_dir().join("recent.conf");
    std::fs::remove_file(&conf).unwrap();
    std::fs::create_dir(&conf).unwrap();
    click(&window, (CROSS_X, row_y(3, 0)));
    click(&window, ACCEPT_FILE);
    check(
        "ошибка записи списка показана, список в окне не изменился",
        window.get_start_error().contains("Список баз не сохранён") && names(&window).len() == 3,
    );
    check(
        "при ошибке записи логин и файл базы остаются",
        remembered_login(&rows[0].path).is_some() && Path::new(&file0).exists(),
    );

    println!("\nитог: {failed} провалов");
    if failed > 0 {
        std::process::exit(1);
    }
}
