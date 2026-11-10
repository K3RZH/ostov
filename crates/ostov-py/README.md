# ostov-py

Слой: Надстройки.

Логика документов на Python (ADR-003, ADR-018): `config/documents/<документ>.py` с функциями `before_write(doc, ctx)` (необязательна) и `on_post(doc, ctx)` (обязательна у документа с регистрами). `PyRules::load(config_dir, &model)` загружает их, а `PyRules` встаёт в ядро вместо встроенных правил: `LocalConnector::new(model).with_rules(Arc::new(rules))`.

PyO3 (`=0.30.1`) подключается фичей `python`, по умолчанию выключенной: без неё крейт собирается без установленного Python, а `PyRules::load` возвращает ошибку «сборка без Python».

- Модуль `ostov` (`src/runtime.py`) встроен и лежит в `sys.modules`: `ostov.UserError`, документ `doc` (поля атрибутами, `doc.lines` — кортеж строк, в `on_post` всё только для чтения), движения `ctx.receipt` / `ctx.issue` с ключами ровно из измерений и ресурсов регистра. Между Rust и Python ходят только `None`, `int` и `str`; `Decimal` и даты собираются в модуле `ostov` по типам полей, `float` в учётном поле — `TypeError`.
- Чтение базы в `on_post`: `ctx.query("SELECT … WHERE goods = :g", g=row.goods)` — запрос через `ostov-sql`, в той же транзакции и с правами того, кто проводит; строки с полями атрибутами, деньги `Decimal`, даты `datetime.date`, ссылки id. `on_post` идёт в своём потоке, а поток ядра выполняет его запросы: соединение занято транзакцией, а PyO3 берёт только `'static`-функции. После `on_post` `ctx.query` закрыт. Нет права — `UserError`, ошибка в запросе — `ostov.QueryError`.
- Ошибки: `ostov.UserError` → `CoreError::Invalid` с текстом как есть; любое другое исключение → `CoreError::Script` («Ошибка в логике документа «…»: Тип: текст» и трассировка). Ошибки загрузки — `PyLoadError`, «файл:строка — причина».
- Интерпретатор один на процесс. Перед запуском проверяется `Lib\os.py` рядом с программой или в `PYTHONHOME`: без него Python завершил бы процесс, а так — понятная ошибка «рядом с программой нет Python».

## Сборка и тесты с Python

Нужна сборка Python 3.13 из python-build-standalone (`cpython-3.13.18+20261026-x86_64-pc-windows-msvc-install_only`), распакованная в `<папка>\python`:
```
set PYO3_PYTHON=<папка>\python\python.exe
set PYTHONHOME=<папка>\python
set PATH=<папка>\python;%PATH%
cargo test -p ostov-py --features python
```
`PYO3_PYTHON` нужен при сборке, `PYTHONHOME` и `PATH` — при запуске тестов (рядом с тестовой программой Python нет).

## Поставка

Рядом с программой: `python313.dll`, `python3.dll`, `vcruntime140.dll`, `vcruntime140_1.dll`, папки `Lib` и `DLLs` из той же сборки и её `LICENSE.txt`. Без `Lib\test`, Tk, IDLE и `.pdb` — около 30 МБ. Проба упаковки: `cargo run -p ostov-py --features python --example probe`.
