# ostov-model

Слой: Layer0, ниже ядра.

Общие описания метаданных для `ostov-core` и `ostov-meta`: `FieldKind`, `FieldDef`, описания справочника, документа, регистра и раздела меню, `Config` целиком. Только данные, без SQL и без разбора файлов.

Ответ GetMeta для интерфейса: `Meta::new(&config)` — меню (`MenuSection`, `MenuItem`) и объекты (`ObjectMeta`: справочники и документы с полями и табличными частями). Пример для «Склада»: `cargo run -p ostov-meta --example meta`.

Ядро строит из них свои `Catalog`, `DocumentKind`, `AccumulationRegister`; `ostov-meta` получает их из YAML (`ostov_meta::load`). Модель передаётся в ядро снаружи, ядро не зависит от `ostov-meta`.
