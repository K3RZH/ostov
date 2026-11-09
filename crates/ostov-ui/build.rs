fn main() {
    // Ошибку разметки показываем как есть и завершаем сборку без паники (lint expect_used).
    if let Err(error) = slint_build::compile("ui/app.slint") {
        eprintln!("не удалось собрать интерфейс Slint: {error}");
        std::process::exit(1);
    }
}
