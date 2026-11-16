//! Стенд универсального списка: окну посылаются настоящие события мыши и клавиатуры, а строки
//! приходят из настоящей демо-базы. Проверяет, что пункты «Товары» и «Расход» открывают список,
//! щелчок по шапке сортирует, поиск фильтрует, прокрутка догружает дальние строки, «Открыть»,
//! двойной щелчок и пункт меню открывают форму строки, «Создать» форму нового объекта, а
//! записанная форма обновляет строку списка. Правая кнопка открывает меню строки: пункты по
//! виду объекта, состоянию строки и правам, действие идёт в настоящее ядро. Занятые данные
//! (ответ ядра `Unavailable`) дают ошибку с «Повторить», и повтор выполняет то же действие.
//! Запуск: `env -u WAYLAND_DISPLAY SLINT_BACKEND=winit-software cargo run -p ostov-ui --example object_list_e2e`.
// Стенд, а не приложение: падение на unwrap и есть провал сценария.
#![allow(clippy::unwrap_used)]

#[path = "support/busy.rs"]
mod busy;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use busy::BusyEngine;

use ostov_ui::app::{refresh, wire_tabs};
use ostov_ui::form_ui::FormUi;
use ostov_ui::list_ui::ListUi;
use ostov_ui::shell::Shell;
use ostov_ui::windows::Windows;
use ostov_ui::{AppWindow, FormData, ListData};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, SharedString};

/// Поступлений в демо-базе: расходов по ним выходит больше одного окна списка (100 строк).
const RECEIPTS: usize = 2000;

// Точки окна 1440×900 (снимки `snapshot -- … list-goods`).
const NAV_GOODS: (f32, f32) = (108.0, 176.0);
const NAV_RECEIPT: (f32, f32) = (128.0, 278.0);
const NAV_ISSUE: (f32, f32) = (108.0, 312.0);
const OPEN_BUTTON: (f32, f32) = (350.0, 174.0);
const CREATE_BUTTON: (f32, f32) = (443.0, 174.0);
/// «Записать» на форме: слева под заголовком, при ширине окна не сдвигается.
const FORM_SAVE_BUTTON: (f32, f32) = (353.0, 174.0);
const SEARCH: (f32, f32) = (1275.0, 174.0);
const FIRST_HEADER: (f32, f32) = (340.0, 227.0);
const DATE_HEADER: (f32, f32) = (395.0, 227.0);
const THIRD_ROW: (f32, f32) = (700.0, 324.0);
const FIRST_ROW: (f32, f32) = (700.0, 260.0);
/// Заголовок экрана: щелчок сюда закрывает открытое меню и ничего больше не делает.
const NEUTRAL: (f32, f32) = (600.0, 140.0);

fn step() {
    std::thread::sleep(std::time::Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
}

fn settle() {
    for _ in 0..12 {
        step();
    }
}

/// Ждёт ответа ядра: фоновые вызовы забирает таймер интерфейса.
fn wait_for(ready: impl Fn() -> bool) -> bool {
    for _ in 0..250 {
        if ready() {
            return true;
        }
        step();
    }
    ready()
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

fn right_click(w: &AppWindow, (x, y): (f32, f32)) {
    settle();
    let position = LogicalPosition::new(x, y);
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    w.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Right,
    });
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Right,
    });
    settle();
}

/// Два нажатия подряд в одной точке: Slint считает их двойным щелчком.
fn double_click(w: &AppWindow, (x, y): (f32, f32)) {
    settle();
    let position = LogicalPosition::new(x, y);
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    for _ in 0..2 {
        w.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        w.window().dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }
    settle();
}

fn scroll(w: &AppWindow, (x, y): (f32, f32), delta_y: f32) {
    let position = LogicalPosition::new(x, y);
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    w.window().dispatch_event(WindowEvent::PointerScrolled {
        position,
        delta_x: 0.0,
        delta_y,
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

/// Ячейки строки списка, как их видит таблица; у строки, которая ещё не пришла, пусто.
fn cells(w: &AppWindow, row: usize) -> Vec<String> {
    w.global::<ListData>()
        .get_rows()
        .row_data(row)
        .map(|row| row.cells.iter().map(|cell| cell.to_string()).collect())
        .unwrap_or_default()
}

fn cell(w: &AppWindow, row: usize, column: usize) -> String {
    cells(w, row).get(column).cloned().unwrap_or_default()
}

/// Подписи пунктов меню строки, как их увидит таблица.
fn menu_labels(w: &AppWindow) -> Vec<String> {
    w.global::<ListData>()
        .get_menu_actions()
        .iter()
        .map(|action| action.label.to_string())
        .collect()
}

fn menu_enabled(w: &AppWindow) -> Vec<bool> {
    w.global::<ListData>()
        .get_menu_actions()
        .iter()
        .map(|action| action.enabled)
        .collect()
}

fn row_muted(w: &AppWindow, row: usize) -> bool {
    w.global::<ListData>()
        .get_rows()
        .row_data(row)
        .is_some_and(|row| row.muted)
}

fn titles(w: &AppWindow) -> Vec<String> {
    w.global::<ListData>()
        .get_columns()
        .iter()
        .map(|column| column.title.to_string())
        .collect()
}

fn sort_dirs(w: &AppWindow) -> Vec<i32> {
    w.global::<ListData>().get_sort_dirs().iter().collect()
}

fn summary(w: &AppWindow) -> String {
    w.global::<ListData>().get_summary().to_string()
}

/// Дата «дд.мм.гггг» ключом для сравнения.
fn date_key(text: &str) -> (u32, u32, u32) {
    let mut parts = text.split('.').map(|part| part.parse().unwrap_or(0));
    let (day, month, year) = (parts.next(), parts.next(), parts.next());
    (year.unwrap_or(0), month.unwrap_or(0), day.unwrap_or(0))
}

fn main() {
    let path = std::env::temp_dir().join(format!(
        "ostov-object-list-e2e-{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let engine = ostov_core::LocalConnector::default()
        .create_demo(path.to_str().unwrap(), RECEIPTS)
        .unwrap();

    let window = AppWindow::new().unwrap();
    ostov_ui::date_input::install(&window);
    window.set_page(2);
    window
        .window()
        .set_size(slint::LogicalSize::new(1440.0, 900.0));
    window.show().unwrap();

    let meta = ostov_api::Meta::new(&ostov_meta::demo().unwrap());
    let shell = Rc::new(RefCell::new(Shell::empty()));
    let receipts = ostov_ui::receipts_ui::setup(&window, shell.clone());
    let windows = Windows::new(&receipts);
    *shell.borrow_mut() = Shell::from_meta(&meta, ostov_api::SystemRights::ALL);
    refresh(&window, &shell.borrow());
    receipts.attach(engine.clone());
    wire_tabs(&window, &shell, &receipts, &windows);
    // Списки и формы ходят в ядро через подставное: оно передаёт всё настоящему, пока его не
    // попросят ответить «данные заняты».
    let busy = Arc::new(BusyEngine::new(engine.clone()));
    let forms = FormUi::new(window.as_weak(), windows.clone(), shell.clone());
    forms.attach_window(&window);
    forms.start(busy.clone(), &meta.objects);
    let lists = ListUi::new(window.as_weak(), windows, forms.clone());
    lists.attach_window(&window);
    lists.start(busy.clone(), &meta.objects);
    settle();

    let mut failed = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "✓" } else { "✗" });
        if !ok {
            failed += 1;
        }
    };
    let data = window.global::<ListData>();
    let active_view = || {
        let shell = shell.borrow();
        shell.tabs()[shell.active_tab()].view
    };

    // ---------------------------------------------------------------- справочник
    click(&window, NAV_GOODS);
    check("«Товары» открывает вкладку списка", active_view() == "list");
    check(
        "заголовок экрана из метаданных",
        data.get_title() == "Товары",
    );
    check(
        "строки пришли из ядра: «Показано 8 из 8»",
        wait_for(|| summary(&window) == "Показано 8 из 8"),
    );
    check(
        "колонки справочника по метаданным",
        titles(&window) == ["Наименование", "Единица измерения"],
    );
    check(
        "в ячейках данные базы",
        cells(&window, 0) == ["Мука пшеничная в/с", "кг"],
    );

    click(&window, FIRST_HEADER);
    check(
        "щелчок по шапке сортирует по возрастанию",
        sort_dirs(&window) == [1, 0] && wait_for(|| cell(&window, 0, 0) == "Говядина охлаждённая"),
    );
    click(&window, FIRST_HEADER);
    check(
        "второй щелчок: по убыванию",
        sort_dirs(&window) == [2, 0] && wait_for(|| cell(&window, 0, 0) == "Яйцо куриное С1"),
    );
    click(&window, FIRST_HEADER);
    check(
        "третий щелчок снимает сортировку",
        sort_dirs(&window) == [0, 0] && wait_for(|| cell(&window, 0, 0) == "Мука пшеничная в/с"),
    );

    // ---------------------------------------------------------------- меню строки справочника
    right_click(&window, FIRST_ROW);
    check(
        "правая кнопка выбирает строку и открывает меню справочника",
        data.get_selected_row() == 0
            && menu_labels(&window) == ["Открыть", "Пометить на удаление", "Обновить список"],
    );
    data.invoke_action(0, "delete".into());
    check(
        "пометка на удаление сначала спрашивает подтверждение",
        data.get_confirm_open()
            && data.get_confirm_title() == "Пометить на удаление?"
            && data.get_confirm_text().starts_with("Мука пшеничная в/с"),
    );
    data.invoke_confirm_cancelled();
    settle();
    check(
        "«Отмена» закрывает вопрос и не трогает строку",
        !data.get_confirm_open() && !row_muted(&window, 0),
    );
    click(&window, NEUTRAL);
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "delete".into());
    data.invoke_confirm_accepted();
    check(
        "после подтверждения строка помечена и приглушена",
        !data.get_confirm_open() && wait_for(|| row_muted(&window, 0)),
    );
    click(&window, NEUTRAL);
    right_click(&window, FIRST_ROW);
    check(
        "у помеченной строки пункт «Снять пометку»",
        menu_labels(&window) == ["Открыть", "Снять пометку", "Обновить список"],
    );
    data.invoke_action(0, "restore".into());
    check(
        "пометка снимается сразу, без вопроса",
        !data.get_confirm_open() && wait_for(|| !row_muted(&window, 0)),
    );
    check("ошибки действия нет", data.get_notice() == "");
    click(&window, NEUTRAL);

    // ---------------------------------------------------------------- форма из списка
    let form = window.global::<FormData>();
    let forms_open = || {
        shell
            .borrow()
            .tabs()
            .iter()
            .filter(|tab| tab.view == "form")
            .count()
    };
    let name_in_engine = |id: i64| match engine.get("goods", id).unwrap().fields["name"].clone() {
        ostov_api::Value::Text(text) => text,
        other => unreachable!("{other:?}"),
    };
    let first_name = name_in_engine(1);
    data.set_selected_row(-1);
    click(&window, OPEN_BUTTON);
    check(
        "«Открыть» без выбранной строки ничего не открывает",
        forms_open() == 0,
    );
    check(
        "«Создать» есть: у пользователя есть право создания",
        data.get_can_create(),
    );
    click(&window, FIRST_ROW);
    click(&window, OPEN_BUTTON);
    check(
        "«Открыть» открывает форму выбранной строки во вкладке",
        wait_for(|| forms_open() == 1 && active_view() == "form" && form.get_title() == first_name),
    );
    settle();
    type_text(&window, "Я");
    click(&window, FORM_SAVE_BUTTON);
    check(
        "форма записала объект в ядро",
        wait_for(|| name_in_engine(1) == format!("Я{first_name}") && !form.get_modified()),
    );
    form.invoke_close();
    settle();
    check(
        "после записи строка в открытом списке обновилась на месте",
        active_view() == "list" && cell(&window, 0, 0) == format!("Я{first_name}"),
    );
    // Вернули имя: следующие проверки ждут прежние данные.
    let mut back = engine.get("goods", 1).unwrap();
    back.fields.insert(
        "name".to_owned(),
        ostov_api::Value::Text(first_name.clone()),
    );
    engine
        .save("goods", &back, ostov_api::PostAction::Write)
        .unwrap();
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "reload".into());
    check(
        "«Обновить список» вернул прежнее имя",
        wait_for(|| cell(&window, 0, 0) == first_name),
    );
    click(&window, NEUTRAL);

    double_click(&window, FIRST_ROW);
    check(
        "двойной щелчок по строке открывает её форму",
        wait_for(|| forms_open() == 1 && form.get_title() == first_name),
    );
    form.invoke_close();
    settle();
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "open".into());
    check(
        "пункт «Открыть» в меню строки открывает форму",
        wait_for(|| forms_open() == 1 && form.get_title() == first_name),
    );
    form.invoke_close();
    settle();
    // Меню закрывается щелчком вне его: событие пункта шло в обход всплывающего окна.
    click(&window, NEUTRAL);

    // Отказ открыть форму ложится полосой над таблицей, как ошибка действия.
    forms.start(engine.clone(), &[]);
    click(&window, FIRST_ROW);
    click(&window, OPEN_BUTTON);
    check(
        "отказ открыть форму виден над таблицей с названием строки",
        data.get_notice() == format!("{first_name}: Такого объекта нет в конфигурации базы.")
            && forms_open() == 0,
    );
    data.invoke_dismiss_notice();
    forms.start(engine.clone(), &meta.objects);

    click(&window, CREATE_BUTTON);
    check(
        "«Создать» открывает форму нового объекта",
        wait_for(|| forms_open() == 1 && form.get_title() == "Товары (новый)"),
    );
    settle();
    type_text(&window, "Соль");
    click(&window, FORM_SAVE_BUTTON);
    check(
        "новый объект записан, вкладка получила настоящий id",
        wait_for(|| {
            shell.borrow().tabs().iter().any(|tab| {
                tab.view == "form" && tab.doc.as_ref().is_some_and(|d| !d.is_temporary())
            })
        }) && form.get_title() == "Соль",
    );
    form.invoke_close();
    check(
        "новый объект появился в списке: «Показано 9 из 9»",
        wait_for(|| summary(&window) == "Показано 9 из 9") && cell(&window, 8, 0) == "Соль",
    );

    click(&window, SEARCH);
    type_text(&window, "мас");
    check("текст поиска вводится", data.get_query() == "мас");
    check(
        "поиск фильтрует: «Показано 1 из 9»",
        wait_for(|| summary(&window) == "Показано 1 из 9")
            && cells(&window, 0) == ["Масло сливочное 82%", "кг"],
    );

    // ---------------------------------------------------------------- документ
    click(&window, NAV_ISSUE);
    check(
        "«Расход» открывает свой список в той же вкладке",
        active_view() == "list" && data.get_title() == "Расход" && data.get_object() == "issue",
    );
    check(
        "колонки документа: номер, дата, поля шапки, статус",
        titles(&window) == ["№", "Дата", "Склад", "Статус"],
    );
    check(
        "у нового списка свой поиск и своя сортировка",
        data.get_query() == "" && sort_dirs(&window) == [0, 0, 0, 0],
    );
    check(
        "расходы пришли из ядра",
        wait_for(|| summary(&window).starts_with("Показано ")),
    );
    let total = data.get_rows().row_count();
    check(
        "расходов больше одного окна списка",
        total > 100 && summary(&window) == format!("Показано {total} из {total}"),
    );

    click(&window, DATE_HEADER);
    click(&window, DATE_HEADER);
    check(
        "дата по убыванию: значок в шапке",
        sort_dirs(&window) == [0, 2, 0, 0],
    );
    let sorted = wait_for(|| !cell(&window, 0, 1).is_empty()) && {
        let dates: Vec<_> = (0..15)
            .map(|row| date_key(&cell(&window, row, 1)))
            .collect();
        dates.windows(2).all(|pair| pair[0] >= pair[1])
    };
    check("строки идут от поздней даты к ранней", sorted);

    // ---------------------------------------------------------------- меню строки документа
    let status = |row| cell(&window, row, 3);
    check("последний расход проведён", status(0) == "Проведён");
    right_click(&window, FIRST_ROW);
    check(
        "у проведённого документа «Отменить проведение», пометка недоступна",
        menu_labels(&window)
            == [
                "Открыть",
                "Отменить проведение",
                "Пометить на удаление",
                "Обновить список",
            ]
            && menu_enabled(&window) == [true, true, false, true],
    );
    data.invoke_action(0, "unpost".into());
    check(
        "проведение отменено в ядре: статус «Записан»",
        wait_for(|| status(0) == "Записан") && data.get_notice() == "",
    );
    click(&window, NEUTRAL);
    right_click(&window, FIRST_ROW);
    check(
        "у записанного документа «Провести» и доступная пометка",
        menu_labels(&window)
            == [
                "Открыть",
                "Провести",
                "Пометить на удаление",
                "Обновить список",
            ]
            && menu_enabled(&window) == [true, true, true, true],
    );
    data.invoke_action(0, "post".into());
    check(
        "документ проведён снова",
        wait_for(|| status(0) == "Проведён") && data.get_notice() == "",
    );
    click(&window, NEUTRAL);
    // Отказ ядра: пометить проведённый документ нельзя. Меню такой пункт не даёт нажать, поэтому
    // событие шлём напрямую, как если бы оно пришло в обход.
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "delete".into());
    data.invoke_confirm_accepted();
    check(
        "отказ ядра виден над таблицей с названием строки, строка не тронута",
        wait_for(|| data.get_notice().starts_with("Расход № "))
            && data.get_notice().contains("сначала отмените проведение")
            && !row_muted(&window, 0)
            && status(0) == "Проведён",
    );
    data.invoke_dismiss_notice();
    check("крестик убирает ошибку", data.get_notice() == "");
    click(&window, NEUTRAL);
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "reload".into());
    check(
        "«Обновить список» читает список заново с первой строки",
        data.get_selected_row() == -1
            && wait_for(|| summary(&window) == format!("Показано {total} из {total}"))
            && wait_for(|| status(0) == "Проведён"),
    );
    click(&window, NEUTRAL);

    // Данные заняты другим пользователем: ядро уже повторило само и сдалось. У ошибки над
    // таблицей кнопка «Повторить», она выполняет то же действие над той же строкой.
    busy.fail_next(1);
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "unpost".into());
    check(
        "занятые данные: ошибка с названием строки и «Повторить», строка не тронута",
        wait_for(|| data.get_notice().contains("меняет другой пользователь"))
            && data.get_notice().starts_with("Расход № ")
            && data.get_notice_action() == "Повторить"
            && status(0) == "Проведён",
    );
    data.invoke_notice_retry();
    check(
        "«Повторить» снимает проведение той же строки, ошибка уходит",
        wait_for(|| status(0) == "Записан")
            && data.get_notice().is_empty()
            && data.get_notice_action().is_empty(),
    );
    click(&window, NEUTRAL);
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "post".into());
    check(
        "документ проведён обратно",
        wait_for(|| status(0) == "Проведён") && data.get_notice().is_empty(),
    );
    // Крестик убирает ошибку вместе с повтором: строка осталась проведённой.
    busy.fail_next(1);
    click(&window, NEUTRAL);
    right_click(&window, FIRST_ROW);
    data.invoke_action(0, "unpost".into());
    check(
        "после крестика повторять нечего, строка проведена",
        wait_for(|| data.get_notice_action() == "Повторить")
            && {
                data.invoke_dismiss_notice();
                data.get_notice().is_empty() && data.get_notice_action().is_empty()
            }
            && status(0) == "Проведён",
    );
    click(&window, NEUTRAL);

    // Прокрутка далеко за первое окно: строки грузит сама таблица по видимой области. Стенд их
    // не запрашивает: первый же вопрос к модели должен застать строку загруженной.
    scroll(&window, THIRD_ROW, -32.0 * 150.0);
    for _ in 0..60 {
        step();
    }
    check(
        "прокрутка догружает дальние строки",
        !cell(&window, 160, 0).is_empty(),
    );

    // ---------------------------------------------------------------- возврат и старый экран
    click(&window, NAV_GOODS);
    check(
        "список товаров помнит поиск",
        data.get_title() == "Товары"
            && data.get_query() == "мас"
            && wait_for(|| summary(&window) == "Показано 1 из 9"),
    );
    click(&window, NAV_RECEIPT);
    check(
        "«Поступление» остаётся на старом экране",
        active_view() == "receipts",
    );

    // ---------------------------------------------------------------- права
    let restricted: Vec<_> = meta
        .objects
        .iter()
        .map(|object| ostov_api::ObjectMeta {
            rights: ostov_api::Rights {
                post: false,
                unpost: false,
                delete: false,
                create: false,
                ..ostov_api::Rights::ALL
            },
            ..object.clone()
        })
        .collect();
    lists.start(engine.clone(), &restricted);
    forms.start(engine.clone(), &restricted);
    click(&window, NAV_GOODS);
    check(
        "без прав на проведение и пометку в меню «Открыть» (право чтения) и «Обновить список»",
        wait_for(|| summary(&window) == "Показано 9 из 9")
            && {
                right_click(&window, FIRST_ROW);
                menu_labels(&window) == ["Открыть", "Обновить список"]
            }
            && !data.get_can_create(),
    );
    click(&window, NEUTRAL);

    lists.clear();
    forms.clear();
    drop(engine);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite.lock"));
    println!("итог: {failed} провалов");
    if failed > 0 {
        std::process::exit(1);
    }
}
