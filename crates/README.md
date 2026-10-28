# crates

Крейты платформы (один Cargo workspace). Зависимости идут вниз: интерфейс и сервер знают только контракт `ostov-api`, ядро `ostov-core` ничего не знает ни про интерфейс, ни про сеть.

| Слой | Крейты |
|---|---|
| Layer0 — нижний слой ядра: хранение, транзакции, проведение, контракт, сеть | `ostov-store`, `ostov-core`, `ostov-api`, `ostov-server`, `ostov-client` |
| Надстройки — крейты на Rust поверх Layer0 | `ostov-meta`, `ostov-sql`, `ostov-py`, `ostov-sec`, `ostov-io`, `ostov-cli` |
| Клиент — интерфейс | `ostov-table`, `ostov-ui` |

Режим «один компьютер»: `ostov-ui` → `ostov-core` в том же процессе, база SQLite.
Режим «сервер в локальной сети»: `ostov-ui` → `ostov-client` → gRPC → `ostov-server` → `ostov-core`, база PostgreSQL.
