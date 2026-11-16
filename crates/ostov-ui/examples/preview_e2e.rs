//! Стенд предпросмотра: клиент с `--preview --config <папка> --base <файл>` открывает тестовую
//! базу сразу и строит меню из конфигурации этой папки. Папка — копия `config/` с другим
//! заголовком справочника «Товары»: если новый заголовок виден в дереве навигации, значит
//! встроенная «Склад» (`ostov_meta::demo`) не используется. Ещё проверяется метка
//! «Предпросмотр, тестовая база», что список недавних баз не тронут и что битая папка даёт
//! ошибку в окне. Файлы настроек стенд пишет в свою временную папку (`XDG_CONFIG_HOME`), как
//! `start_e2e`.
//! Запуск: `env -u WAYLAND_DISPLAY SLINT_BACKEND=winit-software cargo run -p ostov-ui --example preview_e2e [снимок.ppm]`.
// Стенд, а не приложение: падение на unwrap и есть провал сценария.
#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ostov_ui::AppWindow;
use slint::{ComponentHandle, Model};

const CHILD_MARK: &str = "OSTOV_PREVIEW_E2E_CHILD";
const NEW_TITLE: &str = "Товары предпросмотра";

/// Родитель: своя папка настроек, стенд в ней, код возврата стенда.
fn run_in_temp_config() -> ! {
    let dir = std::env::temp_dir().join(format!("ostov-preview-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("settings")).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(std::env::args_os().skip(1))
        .env("XDG_CONFIG_HOME", dir.join("settings"))
        .env(CHILD_MARK, &dir)
        .status()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    std::process::exit(status.code().unwrap_or(1));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Копия `config/` с новым заголовком справочника «Товары»; в пути кириллица и пробел.
fn changed_config(work: &Path) -> PathBuf {
    let dir = work.join("Конфигурация проба");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config"),
        &dir,
    );
    let goods = dir.join("catalogs").join("goods.yaml");
    let text = std::fs::read_to_string(&goods).unwrap();
    assert!(
        text.contains("title: Товары\n"),
        "в goods.yaml нет заголовка"
    );
    std::fs::write(
        &goods,
        text.replacen("title: Товары\n", &format!("title: {NEW_TITLE}\n"), 1),
    )
    .unwrap();
    dir
}

/// Крутит таймеры, пока не выполнится условие: фоновые вызовы ядра отвечают через таймер.
fn wait_until(w: &AppWindow, done: impl Fn(&AppWindow) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        slint::platform::update_timers_and_animations();
        if done(w) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn nav_titles(w: &AppWindow) -> Vec<String> {
    w.get_nav()
        .iter()
        .map(|row| row.title.to_string())
        .collect()
}

fn save_snapshot(w: &AppWindow, path: &str) {
    let snapshot = w.window().take_snapshot().unwrap();
    let mut file = std::fs::File::create(path).unwrap();
    write!(
        file,
        "P6\n{} {}\n255\n",
        snapshot.width(),
        snapshot.height()
    )
    .unwrap();
    for pixel in snapshot.as_slice() {
        file.write_all(&[pixel.r, pixel.g, pixel.b]).unwrap();
    }
    println!("снимок: {path}");
}

fn main() {
    let Some(work) = std::env::var_os(CHILD_MARK).map(PathBuf::from) else {
        run_in_temp_config();
    };
    let mut failed = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "✓" } else { "✗" });
        if !ok {
            failed += 1;
        }
    };

    let dir = changed_config(&work);
    let base = dir.join(".ostov-test").join("test.sqlite");
    let config = ostov_ui::local_base::Config::load(&dir).unwrap();
    drop(config.create(&base.to_string_lossy(), None, None).unwrap());

    let launch = ostov_ui::launch::parse(ostov_ui::launch::preview_args(&dir, &base)).unwrap();
    let window = ostov_ui::app::start(&launch).unwrap();
    window.show().unwrap();
    check(
        "до ответа ядра виден предпросмотр, а не выбор базы",
        window.get_preview() && window.get_page() == 0,
    );
    let opened = wait_until(&window, |w| w.get_page() == 2);
    check("тестовая база открылась сразу, без окна выбора", opened);
    let titles = nav_titles(&window);
    check(
        "в дереве навигации заголовок из папки, а не встроенный",
        titles.iter().any(|title| title == NEW_TITLE) && !titles.iter().any(|t| t == "Товары"),
    );
    check(
        "метка «Предпросмотр, тестовая база» в строке состояния",
        window.get_mode_text() == "Предпросмотр, тестовая база",
    );
    check(
        "база названа по конфигурации",
        window.get_base_name() == "Склад",
    );
    if let Some(path) = std::env::args().nth(1) {
        window
            .window()
            .set_size(slint::LogicalSize::new(1440.0, 900.0));
        wait_until(&window, |_| true);
        save_snapshot(&window, &path);
    }
    // Закрыть базу в предпросмотре значит закрыть окно: выбора другой базы нет.
    window.invoke_close_base();
    let settings = PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()).join("ostov");
    check(
        "список недавних баз не записан",
        !settings.join("recent.conf").exists(),
    );
    window.hide().unwrap();
    drop(window);

    // Вторая копия держит базу: пока первая не закрыта, файл занят.
    let held = ostov_ui::local_base::Config::load(&dir)
        .unwrap()
        .open(&base.to_string_lossy());
    check("после закрытия база свободна", held.is_ok());
    let busy = ostov_ui::app::start(&launch).unwrap();
    let refused = wait_until(&busy, |w| !w.get_start_error().is_empty());
    check(
        "занятая база: причина в окне предпросмотра",
        refused && busy.get_page() == 0 && busy.get_start_error().contains("открыта"),
    );
    drop(held);
    drop(busy);

    // Битая папка: окно открывается с причиной, а не падает.
    let goods = dir.join("catalogs").join("goods.yaml");
    std::fs::write(&goods, "catalog: goods\ncolour: red\n").unwrap();
    let broken = ostov_ui::app::start(&launch).unwrap();
    check(
        "битая конфигурация: причина с файлом в окне",
        broken.get_start_error().contains("goods.yaml"),
    );

    if failed > 0 {
        println!("провалено проверок: {failed}");
        std::process::exit(1);
    }
    println!("все проверки прошли");
}
