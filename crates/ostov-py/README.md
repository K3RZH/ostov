# ostov-py

Слой: Надстройки.

Встроенный Python и API платформы для конфигураций (ADR-003, ADR-018).

Статус: проба упаковки (шаг 1 ТЗ ostov-py). PyO3 подключается фичей `python`, по умолчанию выключенной: без неё крейт собирается без установленного Python.

## Проба: PyO3 + python-build-standalone

1. Скачать сборку Python 3.13 (`cpython-3.13.18+20261026-x86_64-pc-windows-msvc-install_only.tar.gz` из релиза `20261026` astral-sh/python-build-standalone), сверить SHA256 с `SHA256SUMS`, распаковать.
2. Собрать, указав PyO3 этот Python (нужен только при сборке):
   ```
   set PYO3_PYTHON=<папка>\python\python.exe
   cargo build -p ostov-py --features python --example probe
   ```
3. Рядом с `probe.exe` положить из сборки: `python313.dll`, `python3.dll`, `vcruntime140.dll`, `vcruntime140_1.dll`, папки `Lib` и `DLLs`. Интерпретатор находит стандартную библиотеку по `Lib\os.py` рядом с программой; `PYTHONHOME` не нужен.
4. `probe.exe probe.py` — вызывает функции из `probe.py`.

Минимальная поставка без `Lib\test`, Tk, IDLE и `.pdb` — около 30 МБ.
