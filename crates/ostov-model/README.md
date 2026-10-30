# ostov-model

Слой: Layer0, ниже ядра.

Общие описания метаданных для `ostov-core` и `ostov-meta`: `FieldKind`, `FieldDef`, описания справочника, документа и регистра, `Config` целиком. Только данные, без SQL и без разбора файлов.

Ядро строит из них свои `Catalog`, `DocumentKind`, `AccumulationRegister`; `ostov-meta` получает их из YAML (`ostov_meta::load`). Модель передаётся в ядро снаружи, ядро не зависит от `ostov-meta`.
