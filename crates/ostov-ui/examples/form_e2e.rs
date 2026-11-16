//! Стенд формы объекта: окну посылаются настоящие события мыши и клавиатуры, объекты приходят из
//! настоящей демо-базы. Проверяет открытие формы справочника и документа, набор в первом поле
//! (фокус при открытии), что Enter не записывает, запись кнопкой, конфликт версий и «Перечитать»,
//! новый объект с проверкой обязательного поля, проведение и отмену, закрытие без воскрешения
//! несохранённых правок, ошибку чтения, форму в дополнительном окне. Новый «Расход»: выбор
//! склада мышью и клавиатурой (поиск, стрелки, Enter, Esc, Delete), строка товара в окне строки
//! с выбором товара поверх него, проведение; «+ Добавить» из выбора (новый склад подставляется
//! в поле). «Повторить» после ответа ядра «данные заняты» при проведении.
//! Запуск: `env -u WAYLAND_DISPLAY SLINT_BACKEND=winit-software cargo run -p ostov-ui --example form_e2e`.
// Стенд, а не приложение: падение на unwrap и есть провал сценария.
#![allow(clippy::unwrap_used)]

#[path = "support/busy.rs"]
mod busy;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use busy::BusyEngine;
use ostov_api::{Engine, ListQuery, PostAction, Value};
use ostov_ui::app::{refresh, wire_tabs};
use ostov_ui::form_ui::FormUi;
use ostov_ui::shell::{DocKey, Shell};
use ostov_ui::windows::Windows;
use ostov_ui::{AppWindow, EditorKind, FormData};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, SharedString};

// Точки окна 1200×760 (снимки `snapshot -- … form-catalog`, `form-issue`, `form-row`).
const SAVE_BUTTON: (f32, f32) = (353.0, 174.0);
const FIRST_FIELD: (f32, f32) = (526.0, 263.0);
const ISSUE_WAREHOUSE: (f32, f32) = (950.0, 263.0);
const ADD_LINE: (f32, f32) = (913.0, 356.0);
const FIRST_LINE: (f32, f32) = (700.0, 428.0);
// Поле количества в окне строки: внутри поля и с полосой ошибки «Готово», и без неё.
const ROW_QTY: (f32, f32) = (740.0, 446.0);
const PICKER_ADD: (f32, f32) = (560.0, 594.0);
const POST_BUTTON: (f32, f32) = (353.0, 174.0);
const ISSUE_SAVE_BUTTON: (f32, f32) = (460.0, 174.0);

/// Один такт цикла событий. Безобидное движение мыши заставляет окно создать экраны, которые
/// только что появились: настоящий цикл делает это сам при отрисовке.
fn step(w: &AppWindow) {
    std::thread::sleep(std::time::Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
    w.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(1.0, 1.0),
    });
}

fn settle(w: &AppWindow) {
    for _ in 0..12 {
        step(w);
    }
}

/// Ждёт ответа ядра: фоновые вызовы забирает таймер интерфейса.
fn wait_for(w: &AppWindow, ready: impl Fn() -> bool) -> bool {
    for _ in 0..250 {
        if ready() {
            return true;
        }
        step(w);
    }
    ready()
}

fn click(w: &AppWindow, (x, y): (f32, f32)) {
    settle(w);
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
    settle(w);
}

fn press(w: &AppWindow, text: &str) {
    let text = SharedString::from(text);
    w.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    w.window().dispatch_event(WindowEvent::KeyReleased { text });
    settle(w);
}

fn type_text(w: &AppWindow, text: &str) {
    for ch in text.chars() {
        press(w, &ch.to_string());
    }
}

fn press_key(w: &AppWindow, key: Key) {
    press(w, &char::from(key).to_string());
}

/// `id` и название ссылки из ответа ядра.
fn ref_of(value: &Value) -> (i64, String) {
    let Value::Ref { id, title } = value else {
        unreachable!("не ссылка: {value:?}");
    };
    (*id, title.clone().unwrap_or_default())
}

/// Первые буквы названия строчными: поиск в ядре без учёта регистра.
fn prefix(title: &str) -> String {
    title.chars().take(3).collect::<String>().to_lowercase()
}

fn text_of(engine: &dyn Engine, object: &str, id: i64, field: &str) -> String {
    let fields = engine.get(object, id).unwrap().fields;
    let Some(Value::Text(text)) = fields.get(field) else {
        unreachable!("поле {field}: {:?}", fields.get(field));
    };
    text.clone()
}

fn main() {
    let path = std::env::temp_dir().join(format!("ostov-form-e2e-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let engine = ostov_core::LocalConnector::default()
        .create_demo(path.to_str().unwrap(), 50)
        .unwrap();

    // Формы ходят в ядро через подставное: оно передаёт всё настоящему, пока его не попросят
    // ответить «данные заняты».
    let busy = Arc::new(BusyEngine::new(engine.clone()));

    let window = AppWindow::new().unwrap();
    // Маска и разбор поля даты: в приложении их ставит каждому окну `apply_settings`.
    ostov_ui::date_input::install(&window);
    window.set_page(2);
    window
        .window()
        .set_size(slint::LogicalSize::new(1200.0, 760.0));
    window.show().unwrap();

    let meta = ostov_api::Meta::new(&ostov_meta::demo().unwrap());
    let shell = Rc::new(RefCell::new(Shell::empty()));
    let receipts = ostov_ui::receipts_ui::setup(&window, shell.clone());
    let windows = Windows::new(&receipts);
    *shell.borrow_mut() = Shell::from_meta(&meta, ostov_api::SystemRights::ALL);
    refresh(&window, &shell.borrow());
    wire_tabs(&window, &shell, &receipts, &windows);
    let forms = FormUi::new(window.as_weak(), windows.clone(), shell.clone());
    forms.attach_window(&window);
    windows.on_host_created({
        let forms = forms.clone();
        move |host| forms.attach_window(host)
    });
    forms.start(busy.clone(), &meta.objects);
    settle(&window);

    let mut failed = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "✓" } else { "✗" });
        if !ok {
            failed += 1;
        }
    };
    let data = window.global::<FormData>();
    let active = || {
        let shell = shell.borrow();
        shell.tabs()[shell.active_tab()].clone()
    };
    let field = |index: usize| data.get_fields().row_data(index).unwrap();
    let forms_open = || {
        shell
            .borrow()
            .tabs()
            .iter()
            .filter(|tab| tab.view == "form")
            .count()
    };

    // ---------------------------------------------------------------- справочник
    let flour = DocKey::new("goods", 1);
    let original = text_of(&*engine, "goods", 1, "name");
    check(
        "форму справочника открыть можно",
        forms.open_form(flour.clone()).is_ok(),
    );
    check(
        "вкладка с видом «form», заголовок из ядра",
        wait_for(&window, || {
            active().view == "form" && data.get_title() == original
        }),
    );
    check(
        "вкладка названа по представлению объекта",
        active().title == original && active().doc.as_ref() == Some(&flour),
    );
    check(
        "поля по метаданным: подпись с звёздочкой у обязательного, значения из ядра",
        data.get_fields().row_count() == 2
            && field(0).label == "Наименование *"
            && field(0).text == original
            && field(1).label == "Единица измерения",
    );
    check(
        "справочник: «Записать» есть, «Провести» нет, чипа нет, правок нет",
        data.get_can_save()
            && !data.get_can_post()
            && !data.get_can_unpost()
            && data.get_status().is_empty()
            && !data.get_modified()
            && !data.get_loading()
            && data.get_error().is_empty(),
    );

    let tabs = shell.borrow().tabs().len();
    check(
        "та же форма открывается ещё раз без новой вкладки",
        {
            forms.open_form(flour.clone()).unwrap();
            settle(&window);
            shell.borrow().tabs().len() == tabs
        },
    );

    // Несколько букв подряд: фокус не теряется после первой (форма не пересоздаёт редактор,
    // пока вкладку отмечают изменённой).
    type_text(&window, "ЖЗ");
    check(
        "набор попадает в форму: «изменено», у вкладки точка",
        data.get_modified() && active().modified,
    );
    check(
        "ядру ничего не ушло, пока не нажали «Записать»",
        text_of(&*engine, "goods", 1, "name") == original,
    );
    press(&window, "\n");
    check(
        "Enter в поле не записывает форму",
        text_of(&*engine, "goods", 1, "name") == original && data.get_modified(),
    );

    click(&window, SAVE_BUTTON);
    check(
        "«Записать» кладёт набранное в ядро",
        wait_for(&window, || {
            text_of(&*engine, "goods", 1, "name").starts_with("ЖЗ") && !data.get_modified()
        }),
    );
    let saved_name = text_of(&*engine, "goods", 1, "name");
    check(
        "после записи вкладка без точки, форма показывает записанное",
        !active().modified && field(0).text == saved_name && data.get_error().is_empty(),
    );

    // Конфликт: кто-то записал тот же элемент, пока форма была открыта.
    let mut behind = engine.get("goods", 1).unwrap();
    behind
        .fields
        .insert("unit".to_owned(), Value::Text("мешок".to_owned()));
    engine.save("goods", &behind, PostAction::Write).unwrap();
    click(&window, FIRST_FIELD);
    type_text(&window, "?!");
    click(&window, SAVE_BUTTON);
    check(
        "конфликт версий: ошибка над формой с кнопкой «Перечитать», правки на месте",
        wait_for(&window, || {
            data.get_error().contains("изменён другим пользователем")
        }) && data.get_error_action() == "Перечитать"
            && data.get_modified()
            && !data.get_busy(),
    );
    data.invoke_reload();
    check(
        "«Перечитать» берёт объект из ядра, правки пропадают",
        wait_for(&window, || {
            data.get_error().is_empty() && !data.get_modified()
        }) && field(1).text == "мешок"
            && text_of(&*engine, "goods", 1, "name") == saved_name,
    );

    // Закрытие с несохранёнными правками не оставляет их в памяти. Редакторы после
    // перечитывания созданы заново: фокус в первом поле, набор идёт без щелчка.
    settle(&window);
    type_text(&window, "#");
    check("правка снова изменила форму", data.get_modified());
    data.invoke_close();
    settle(&window);
    check("«Закрыть» закрывает вкладку", forms_open() == 0);
    forms.open_form(flour.clone()).unwrap();
    check(
        "форма, открытая заново, не помнит закрытых правок",
        wait_for(&window, || {
            data.get_title() == saved_name && field(0).text == saved_name
        }) && !data.get_modified(),
    );
    data.invoke_close();
    settle(&window);

    // ---------------------------------------------------------------- крестик вкладки
    let tab_of = |key: &DocKey| {
        shell
            .borrow()
            .tabs()
            .iter()
            .position(|tab| tab.doc.as_ref() == Some(key))
    };
    forms.open_form(flour.clone()).unwrap();
    check(
        "форма для проверки крестика открыта",
        wait_for(&window, || data.get_title() == saved_name),
    );
    window.invoke_tab_closed(i32::try_from(tab_of(&flour).unwrap()).unwrap());
    settle(&window);
    check(
        "крестик вкладки без правок закрывает её без вопроса",
        tab_of(&flour).is_none() && !data.get_ask_close(),
    );
    forms.open_form(flour.clone()).unwrap();
    check(
        "форма открыта заново",
        wait_for(&window, || data.get_title() == saved_name),
    );
    settle(&window);
    type_text(&window, "Ж");
    // Соседняя вкладка на виду: вопрос должен перевести на форму с правками.
    forms.open_form(DocKey::new("goods", 3)).unwrap();
    check(
        "открыта вторая форма, на виду она",
        wait_for(&window, || active().doc == Some(DocKey::new("goods", 3))),
    );
    window.invoke_tab_closed(i32::try_from(tab_of(&flour).unwrap()).unwrap());
    check(
        "крестик вкладки с правками не закрывает её, вкладка выбирается и спрашивает",
        tab_of(&flour).is_some()
            && active().doc.as_ref() == Some(&flour)
            && wait_for(&window, || {
                data.get_title() == saved_name && data.get_ask_close()
            }),
    );
    data.set_ask_close(false);
    data.invoke_ask_cancelled();
    window.invoke_tab_closed(i32::try_from(tab_of(&flour).unwrap()).unwrap());
    check(
        "после «Отмена» вопрос задаётся снова",
        data.get_ask_close() && tab_of(&flour).is_some(),
    );
    // «Закрыть» ответ на вопрос: форма уходит, правки пропадают.
    data.set_ask_close(false);
    data.invoke_close();
    settle(&window);
    check(
        "«Закрыть» в вопросе закрывает вкладку с правками",
        tab_of(&flour).is_none() && text_of(&*engine, "goods", 1, "name") == saved_name,
    );
    // Вопрос относится к одной форме: на другой вкладке его нет.
    forms.open_form(flour.clone()).unwrap();
    check(
        "форма открыта в третий раз",
        wait_for(&window, || data.get_title() == saved_name),
    );
    settle(&window);
    type_text(&window, "З");
    let third = DocKey::new("goods", 3);
    forms.open_form(third.clone()).unwrap();
    check(
        "вторая форма открыта рядом с формой, где есть правки",
        wait_for(&window, || active().doc.as_ref() == Some(&third)),
    );
    window.invoke_tab_closed(i32::try_from(tab_of(&third).unwrap()).unwrap());
    settle(&window);
    check(
        "вторая форма без правок закрылась сразу, спрашивать нечего",
        tab_of(&third).is_none() && tab_of(&flour).is_some(),
    );
    window.invoke_tab_closed(i32::try_from(tab_of(&flour).unwrap()).unwrap());
    check(
        "а у первой вопрос есть: правки не потеряны молча",
        wait_for(&window, || data.get_ask_close()),
    );
    data.set_ask_close(false);
    data.invoke_close();
    settle(&window);
    check("ответ «Закрыть» закрыл и её", tab_of(&flour).is_none());

    // ---------------------------------------------------------------- новый элемент
    let before = engine
        .list(&ListQuery {
            object: "goods".to_owned(),
            size: 1,
            ..ListQuery::default()
        })
        .unwrap()
        .total;
    forms.open_new("goods").unwrap();
    settle(&window);
    check(
        "новый элемент: временный ключ, «Не записан», заголовок по типу объекта",
        active().doc.as_ref().is_some_and(DocKey::is_temporary)
            && data.get_status() == "Не записан"
            && data.get_title() == "Товары (новый)",
    );
    click(&window, SAVE_BUTTON);
    check(
        "пустое обязательное поле не доходит до ядра: ошибка у поля и над формой",
        field(0).error == "Заполните поле." && data.get_error().starts_with("Не записано"),
    );
    // Полосу ошибки сдвинула поля вниз: текст ставится колбэком, настоящий набор проверен выше.
    data.invoke_edited(0, "Соль".into());
    settle(&window);
    check(
        "правка снимает ошибку поля",
        field(0).error.is_empty() && data.get_modified(),
    );
    click(&window, SAVE_BUTTON);
    check(
        "запись нового: вкладка получила настоящий id и название",
        wait_for(&window, || {
            active().doc.as_ref().is_some_and(|doc| !doc.is_temporary())
        }) && active().title == "Соль"
            && !active().modified
            && data.get_status().is_empty()
            && data.get_title() == "Соль",
    );
    let after = engine
        .list(&ListQuery {
            object: "goods".to_owned(),
            size: 1,
            ..ListQuery::default()
        })
        .unwrap()
        .total;
    check(
        "в ядре на один элемент больше",
        after
            .zip(before)
            .is_some_and(|(after, before)| after == before + 1),
    );
    data.invoke_close();
    settle(&window);

    // ---------------------------------------------------------------- документ
    let draft = engine
        .list(&ListQuery {
            object: "receipt".to_owned(),
            posted: Some(false),
            deleted: Some(false),
            size: 1,
            ..ListQuery::default()
        })
        .unwrap()
        .rows[0]
        .id;
    forms.open_form(DocKey::new("receipt", draft)).unwrap();
    check(
        "документ: чип «Черновик», «Провести» и «Записать», без «Отменить проведение»",
        wait_for(&window, || data.get_status() == "Черновик")
            && data.get_can_post()
            && data.get_can_save()
            && !data.get_can_unpost()
            && !data.get_posted(),
    );
    check(
        "поля документа: дата и ссылки для выбора, вычисляемая сумма только для чтения",
        data.get_fields().row_count() == 4
            && !field(0).read_only
            && field(1).kind == EditorKind::Reference
            && !field(1).read_only
            && !field(2).read_only
            && field(3).read_only,
    );
    check(
        "табличная часть «Товары» со строками из ядра",
        data.get_parts().row_count() == 1
            && data.get_parts().row_data(0).unwrap().rows.row_count() > 0,
    );
    // Фокус в дате, первом редактируемом поле: дата набирается цифрами без щелчка.
    type_text(&window, "05032027");
    check(
        "дата набрана в поле даты: форма изменена",
        data.get_modified() && active().modified,
    );
    // Данные заняты: ядро уже повторило само и сдалось. «Повторить» проводит заново.
    busy.fail_next(1);
    data.invoke_post();
    check(
        "занятые данные при проведении: ошибка над формой с «Повторить», правки на месте",
        wait_for(&window, || {
            data.get_error().contains("меняет другой пользователь")
        }) && data.get_error_action() == "Повторить"
            && data.get_modified()
            && !data.get_busy()
            && !engine.get("receipt", draft).unwrap().posted,
    );
    data.invoke_notice_action();
    check(
        "«Повторить» записывает и проводит тем же действием одним вызовом",
        wait_for(&window, || {
            data.get_posted() && data.get_status() == "Проведён" && !data.get_modified()
        }) && data.get_error().is_empty(),
    );
    let stored = engine.get("receipt", draft).unwrap();
    check(
        "в ядре документ проведён, дата и строки на месте",
        stored.posted
            && stored.fields["date"] == Value::Date("2027-03-05".to_owned())
            && !stored.tables["lines"].is_empty(),
    );
    check(
        "у проведённого «Записать» нет, есть «Отменить проведение»",
        !data.get_can_save() && data.get_can_unpost() && data.get_can_post(),
    );
    data.invoke_unpost();
    check(
        "«Отменить проведение» возвращает черновик",
        wait_for(&window, || {
            !data.get_posted() && data.get_status() == "Черновик"
        }) && !engine.get("receipt", draft).unwrap().posted,
    );

    data.invoke_close();
    settle(&window);

    // ---------------------------------------------------------------- расход: выбор и строки
    // Склад и товар из проведённого поступления: остатка по ним хватит на небольшой расход.
    let stocked = engine
        .list(&ListQuery {
            object: "receipt".to_owned(),
            posted: Some(true),
            deleted: Some(false),
            size: 1,
            ..ListQuery::default()
        })
        .unwrap()
        .rows[0]
        .id;
    let stocked = engine.get("receipt", stocked).unwrap();
    let (warehouse_id, warehouse_name) = ref_of(&stocked.fields["warehouse"]);
    let (goods_id, goods_name) = ref_of(&stocked.tables["lines"][0]["goods"]);
    let part = || data.get_parts().row_data(0).unwrap();
    let line = |row: usize| -> Vec<String> {
        part()
            .rows
            .row_data(row)
            .unwrap()
            .cells
            .iter()
            .map(|cell| cell.to_string())
            .collect()
    };
    let row_field = |index: usize| data.get_row_fields().row_data(index).unwrap();
    // Товаров в ядре: выше стенд добавил свой.
    let goods_total = engine
        .list(&ListQuery {
            object: "goods".to_owned(),
            deleted: Some(false),
            size: 1,
            ..ListQuery::default()
        })
        .unwrap()
        .total
        .and_then(|total| usize::try_from(total).ok())
        .unwrap();
    let picker_titles = || -> Vec<String> {
        data.get_picker_items()
            .iter()
            .map(|item| item.title.to_string())
            .collect()
    };

    forms.open_new("issue").unwrap();
    settle(&window);
    check(
        "новый расход: дата, склад для выбора, пустая табличная часть «Товары»",
        data.get_fields().row_count() == 2
            && field(1).kind == EditorKind::Reference
            && !field(1).read_only
            && data.get_parts().row_count() == 1
            && part().title == "Товары"
            && part().rows.row_count() == 0
            && part().editable,
    );
    type_text(&window, "30122026");
    check("дата набрана в первом поле", field(0).text == "30.12.2026");

    click(&window, ISSUE_WAREHOUSE);
    check(
        "щелчок по складу открывает выбор: все склады из ядра, фокус в поиске",
        wait_for(&window, || {
            data.get_picker_open() && data.get_picker_items().row_count() == 3
        }) && data.get_picker_title() == "Выбор: Склады"
            && !data.get_picker_can_clear()
            && data.get_picker_can_add(),
    );
    type_text(&window, &prefix(&warehouse_name));
    check(
        "поиск по первым буквам в ядре оставляет один склад",
        wait_for(&window, || picker_titles() == [warehouse_name.clone()])
            && data.get_picker_query() == prefix(&warehouse_name)
            && data.get_picker_current() == 0,
    );
    press(&window, "\n");
    check(
        "Enter выбирает выделенный склад, выбор закрыт, форма изменена",
        !data.get_picker_open() && field(1).text == warehouse_name && data.get_modified(),
    );

    // Фокус вернулся в поле склада: клавиатура работает без мыши.
    press(&window, "\n");
    check(
        "Enter в поле склада снова открывает выбор, нынешний склад отмечен и выделен",
        wait_for(&window, || data.get_picker_items().row_count() == 3)
            && data.get_picker_can_clear()
            && {
                let current = usize::try_from(data.get_picker_current()).unwrap();
                let item = data.get_picker_items().row_data(current).unwrap();
                item.current && item.title == warehouse_name
            },
    );
    for _ in 0..3 {
        press_key(&window, Key::UpArrow);
    }
    press_key(&window, Key::DownArrow);
    check(
        "стрелки двигают выделение и не уходят за край",
        data.get_picker_current() == 1,
    );
    press_key(&window, Key::Escape);
    check(
        "Esc закрывает выбор без выбора: склад прежний",
        !data.get_picker_open() && field(1).text == warehouse_name,
    );
    press_key(&window, Key::Delete);
    check("Delete в поле склада очищает его", field(1).text.is_empty());
    press(&window, "\n");
    wait_for(&window, || data.get_picker_items().row_count() == 3);
    type_text(&window, &prefix(&warehouse_name));
    wait_for(&window, || picker_titles() == [warehouse_name.clone()]);
    press(&window, "\n");
    check("склад выбран снова", field(1).text == warehouse_name);

    click(&window, ADD_LINE);
    check(
        "«Добавить» открывает окно новой строки: товар и количество",
        data.get_row_open()
            && data.get_row_title() == "Товары: новая строка"
            && data.get_row_fields().row_count() == 2
            && row_field(0).kind == EditorKind::Reference,
    );
    // Фокус в ячейке товара: Enter открывает выбор поверх окна строки.
    press(&window, "\n");
    check(
        "выбор товара открыт поверх окна строки",
        wait_for(&window, || {
            data.get_picker_open() && data.get_picker_items().row_count() == goods_total
        }) && data.get_picker_title() == "Выбор: Товары"
            && data.get_row_open(),
    );
    type_text(&window, &prefix(&goods_name));
    wait_for(&window, || picker_titles().first() == Some(&goods_name));
    press(&window, "\n");
    check(
        "товар встал в ячейку, окно строки на месте",
        !data.get_picker_open() && data.get_row_open() && row_field(0).text == goods_name,
    );
    click(&window, ROW_QTY);
    press(&window, "\n");
    check(
        "Enter без количества: строка не готова, ошибка у ячейки",
        data.get_row_open()
            && row_field(1).error == "Заполните поле."
            && data.get_row_error().starts_with("Строка не готова"),
    );
    type_text(&window, "0,1");
    press(&window, "\n");
    check(
        "Enter с количеством: строка в таблице, окно закрыто",
        !data.get_row_open()
            && part().rows.row_count() == 1
            && line(0) == [goods_name.clone(), "0,100".to_owned()],
    );

    click(&window, POST_BUTTON);
    check(
        "«Провести»: расход записан и проведён, вкладка с настоящим id",
        wait_for(&window, || data.get_status() == "Проведён")
            && data.get_error().is_empty()
            && !data.get_modified()
            && active().doc.as_ref().is_some_and(|doc| !doc.is_temporary()),
    );
    let issue_id = active().doc.unwrap().id;
    let stored = engine.get("issue", issue_id).unwrap();
    let lines = &stored.tables["lines"];
    check(
        "в ядре: склад, строка с товаром и количеством, документ проведён",
        stored.posted
            && ref_of(&stored.fields["warehouse"]).0 == warehouse_id
            && lines.len() == 1
            && ref_of(&lines[0]["goods"]).0 == goods_id
            && lines[0]["qty"]
                == Value::Decimal {
                    units: 100,
                    scale: 3,
                },
    );
    check(
        "после проведения строка из ядра на месте",
        part().rows.row_count() == 1 && line(0)[0] == goods_name,
    );
    data.invoke_close();
    settle(&window);

    // ---------------------------------------------------------------- «+ Добавить» из выбора
    forms.open_new("issue").unwrap();
    settle(&window);
    let draft = active().doc.clone().unwrap();
    click(&window, ISSUE_WAREHOUSE);
    wait_for(&window, || data.get_picker_items().row_count() == 3);
    click(&window, PICKER_ADD);
    check(
        "«+ Добавить» открывает форму нового склада, вкладка расхода осталась",
        wait_for(&window, || {
            active()
                .doc
                .as_ref()
                .is_some_and(|doc| doc.object == "warehouses" && doc.is_temporary())
        }) && data.get_title() == "Склады (новый)"
            && tab_of(&draft).is_some(),
    );
    settle(&window);
    type_text(&window, "Склад стенда");
    click(&window, SAVE_BUTTON);
    check(
        "склад записан: его вкладка закрыта, расход снова на экране, склад в поле",
        wait_for(&window, || {
            active().doc.as_ref() == Some(&draft) && field(1).text == "Склад стенда"
        }) && shell.borrow().tabs().iter().all(|tab| {
            tab.doc
                .as_ref()
                .is_none_or(|doc| doc.object != "warehouses")
        }) && data.get_modified(),
    );
    check(
        "новый склад есть в ядре",
        engine
            .list(&ListQuery {
                object: "warehouses".to_owned(),
                search: Some("Склад стенда".to_owned()),
                size: 5,
                ..ListQuery::default()
            })
            .unwrap()
            .rows
            .len()
            == 1,
    );

    // Две строки; первая выбирается щелчком и удаляется клавишей Delete.
    for (goods, qty) in [(0, "1"), (1, "2")] {
        data.invoke_row_add(0);
        data.invoke_row_pick(0);
        wait_for(&window, || {
            data.get_picker_items().row_count() == goods_total
        });
        data.invoke_picker_choose(goods);
        data.invoke_row_edited(1, qty.into());
        data.invoke_row_done();
    }
    let second = line(1);
    check("две строки добавлены", part().rows.row_count() == 2);
    click(&window, FIRST_LINE);
    check("щелчок выбирает строку", part().selected == 0);
    press_key(&window, Key::Delete);
    check(
        "Delete удаляет выбранную строку, остаётся вторая",
        part().rows.row_count() == 1 && line(0) == second,
    );

    data.invoke_edited(0, "30.12.2026".into());
    click(&window, ISSUE_SAVE_BUTTON);
    check(
        "«Записать»: черновик с новым складом и строкой",
        wait_for(&window, || data.get_status() == "Черновик")
            && data.get_error().is_empty()
            && active().doc.as_ref().is_some_and(|doc| !doc.is_temporary()),
    );
    let written = engine.get("issue", active().doc.unwrap().id).unwrap();
    check(
        "в ядре черновик: новый склад и одна строка",
        !written.posted
            && ref_of(&written.fields["warehouse"]).1 == "Склад стенда"
            && written.tables["lines"].len() == 1,
    );
    data.invoke_close();
    settle(&window);

    // Объекта нет в ядре: форма не строится, над ней ошибка с кнопкой «Повторить».
    forms.open_form(DocKey::new("goods", 99_999)).unwrap();
    check(
        "нет такого объекта: ошибка чтения и «Повторить», полей нет",
        wait_for(&window, || data.get_error_action() == "Повторить")
            && data.get_error().contains("99999")
            && data.get_fields().row_count() == 0
            && !data.get_loading()
            && !data.get_can_save(),
    );
    data.invoke_close();
    settle(&window);

    // ---------------------------------------------------------------- отдельное окно
    // Вкладка с формой уезжает в дополнительное окно: форма живёт там, набор идёт в видимое поле.
    let second = DocKey::new("goods", 2);
    let second_name = text_of(&*engine, "goods", 2, "name");
    forms.open_form(second.clone()).unwrap();
    check(
        "ещё одна форма открыта",
        wait_for(&window, || data.get_title() == second_name),
    );
    // Правка, набранная до выноса вкладки, переезжает вместе с ней, точка на вкладке остаётся.
    settle(&window);
    type_text(&window, "Щ");
    check("правка перед выносом вкладки", data.get_modified());
    let tab = shell
        .borrow()
        .tabs()
        .iter()
        .position(|tab| tab.doc.as_ref() == Some(&second))
        .unwrap();
    windows.detach_from_main(&window, &shell, tab);
    check(
        "вкладка вынесена в дополнительное окно",
        wait_for(&window, || !windows.debug_state().0.is_empty()),
    );
    let host = windows.host(windows.debug_state().0[0]).unwrap();
    let host_data = host.global::<FormData>();
    check(
        "в дополнительном окне форма показана",
        wait_for(&host, || host_data.get_title() == second_name)
            && host_data.get_fields().row_count() == 2,
    );
    settle(&host);
    let host_id = windows.debug_state().0[0];
    let tab_modified = || {
        windows
            .with_list(host_id, |list| list.tabs()[0].modified)
            .unwrap_or(false)
    };
    check(
        "форма в новом окне помнит набранное, а вкладка свою точку",
        host_data.get_modified() && tab_modified(),
    );
    type_text(&host, "Ы");
    check(
        "набор в дополнительном окне доходит до формы",
        host_data.get_modified() && tab_modified(),
    );
    check(
        "главное окно эту форму не показывает",
        shell.borrow().tabs().iter().all(|tab| tab.view != "form"),
    );
    host.invoke_tab_closed(0);
    settle(&host);
    check(
        "крестик вкладки с правками в дополнительном окне спрашивает, а не закрывает",
        host_data.get_ask_close() && !windows.debug_state().0.is_empty(),
    );
    host_data.set_ask_close(false);
    host_data.invoke_close();
    settle(&host);
    check(
        "«Закрыть» в дополнительном окне закрывает его вкладку и окно",
        wait_for(&window, || windows.debug_state().0.is_empty()),
    );

    drop(host_data);
    drop(data);
    drop(engine);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite.lock"));
    if failed > 0 {
        println!("провалено проверок: {failed}");
        std::process::exit(1);
    }
    println!("все проверки прошли");
}
