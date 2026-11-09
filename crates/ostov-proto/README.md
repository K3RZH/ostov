# ostov-proto

Слой: Layer0.

Сетевой протокол режима 2 (ADR-026): `proto/ostov/v1/engine.proto` — те же команды, что `ostov_api::Engine`, сгенерированный gRPC-код (tonic + prost) и перевод между ним и типами `ostov-api`. Ошибка ядра едет в details gRPC-статуса целиком: клиент получает тот же `EngineError`, что и локально.

Нужен только `ostov-server` и `ostov-client`. `.proto` компилируется через protox — `protoc` ставить не нужно.
