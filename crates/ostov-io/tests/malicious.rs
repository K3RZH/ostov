//! Вредоносные и битые файлы (НТ-Б4): всегда понятная ошибка, процесс жив, посторонние файлы
//! не прочитаны.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::io::Write;
use std::path::Path;

use common::Base;
use ostov_io::{IoError, Limits, read_xlsx, read_xml};
use rust_xlsxwriter::Workbook;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MB: usize = 1024 * 1024;

/// zip с файлами `(имя, содержимое)`, сжатыми deflate.
fn write_zip(path: &Path, files: &[(&str, Vec<u8>)]) {
    let mut zip = ZipWriter::new(std::fs::File::create(path).unwrap());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

/// Псевдослучайные байты: не сжимаются, степень сжатия около 1.
fn noise(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            state.to_be_bytes()[0]
        })
        .collect()
}

fn limit_error(result: Result<ostov_io::Table, IoError>) -> String {
    match result {
        Err(IoError::Limit(reason)) => reason,
        other => panic!("ожидался отказ по лимиту: {other:?}"),
    }
}

#[test]
fn zip_bomb_is_refused_by_table_of_contents() {
    let base = Base::new("bomb");
    let path = base.file("bomb.xlsx");
    // 64 МБ нулей сжимаются до десятков килобайт: степень сжатия около 1000.
    write_zip(&path, &[("xl/worksheets/sheet1.xml", vec![0; 64 * MB])]);
    assert!(
        std::fs::metadata(&path).unwrap().len() < MB as u64,
        "файл маленький"
    );
    let reason = limit_error(read_xlsx(&path, &Limits::default()));
    assert!(reason.contains("похоже на zip-бомбу"), "{reason}");
    assert!(reason.contains("xl/worksheets/sheet1.xml"), "{reason}");
}

#[test]
fn unpacked_size_is_limited() {
    let base = Base::new("unpacked");
    let path = base.file("big.xlsx");
    write_zip(&path, &[("a.xml", noise(3 * MB)), ("b.xml", noise(3 * MB))]);
    let limits = Limits {
        max_unpacked_bytes: 5 * MB as u64,
        ..Limits::default()
    };
    let reason = limit_error(read_xlsx(&path, &limits));
    assert_eq!(reason, "распакованный xlsx больше 5 МБ");
}

#[test]
fn file_size_is_limited() {
    let base = Base::new("file-size");
    let path = base.file("x.xlsx");
    std::fs::write(&path, noise(2 * MB)).unwrap();
    let limits = Limits {
        max_file_bytes: MB as u64,
        ..Limits::default()
    };
    assert_eq!(limit_error(read_xlsx(&path, &limits)), "файл больше 1 МБ");
    let xml = base.file("x.xml");
    std::fs::write(&xml, vec![b' '; 2 * MB]).unwrap();
    assert_eq!(limit_error(read_xml(&xml, &limits)), "файл больше 1 МБ");
}

fn xlsx_with_rows(path: &Path, rows: usize, cell: &str) {
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_string(0, 0, "Наименование").unwrap();
    for row in 1..=u32::try_from(rows).unwrap() {
        sheet.write_string(row, 0, cell).unwrap();
    }
    book.save(path).unwrap();
}

#[test]
fn too_many_rows_are_refused() {
    let base = Base::new("rows");
    let limits = Limits {
        max_rows: 10,
        ..Limits::default()
    };
    let path = base.file("rows.xlsx");
    xlsx_with_rows(&path, 11, "Мука");
    assert_eq!(
        limit_error(read_xlsx(&path, &limits)),
        "строк больше 10: в листе «Sheet1» их 11"
    );
    xlsx_with_rows(&path, 10, "Мука");
    assert_eq!(
        read_xlsx(&path, &limits).unwrap().rows.len(),
        10,
        "ровно лимит — можно"
    );

    let items: String = (0..11).map(|_| "<item><name>Мука</name></item>").collect();
    let xml = base.file("rows.xml");
    std::fs::write(&xml, format!("<goods>{items}</goods>")).unwrap();
    assert_eq!(limit_error(read_xml(&xml, &limits)), "записей больше 10");
}

#[test]
fn too_long_cell_is_refused() {
    let base = Base::new("cell");
    let limits = Limits {
        max_cell_chars: 100,
        ..Limits::default()
    };
    let long = "я".repeat(101);
    let path = base.file("cell.xlsx");
    xlsx_with_rows(&path, 1, &long);
    assert_eq!(
        limit_error(read_xlsx(&path, &limits)),
        "строка 2, колонка 1: ячейка длиннее 100 символов"
    );
    let xml = base.file("cell.xml");
    std::fs::write(
        &xml,
        format!("<goods><item><name>{long}</name></item></goods>"),
    )
    .unwrap();
    assert_eq!(
        limit_error(read_xml(&xml, &limits)),
        "запись 1: значение длиннее 100 символов"
    );
}

#[test]
fn billion_laughs_is_refused() {
    let base = Base::new("laughs");
    let mut dtd = String::from("<!DOCTYPE lolz [<!ENTITY lol \"lol\">");
    for level in 1..10 {
        let previous = if level == 1 {
            "lol".to_owned()
        } else {
            format!("lol{}", level - 1)
        };
        dtd.push_str(&format!(
            "<!ENTITY lol{level} \"{}\">",
            format!("&{previous};").repeat(10)
        ));
    }
    dtd.push(']');
    dtd.push('>');
    let path = base.file("laughs.xml");
    std::fs::write(
        &path,
        format!("<?xml version=\"1.0\"?>{dtd}<goods><item><name>&lol9;</name></item></goods>"),
    )
    .unwrap();
    let error = read_xml(&path, &Limits::default()).unwrap_err();
    assert_eq!(error.to_string(), "XML с DTD не принимается");
}

#[test]
fn external_entity_is_never_read() {
    let base = Base::new("xxe");
    let secret = base.file("secret.txt");
    std::fs::write(&secret, "СЕКРЕТНЫЙ-МАРКЕР-42").unwrap();
    let url = format!(
        "file:///{}",
        secret.display().to_string().replace('\\', "/")
    );
    let path = base.file("xxe.xml");
    std::fs::write(
        &path,
        format!("<!DOCTYPE goods [<!ENTITY xxe SYSTEM \"{url}\">]><goods><item><name>&xxe;</name></item></goods>"),
    )
    .unwrap();
    let error = read_xml(&path, &Limits::default()).unwrap_err().to_string();
    assert_eq!(error, "XML с DTD не принимается");

    // Без DTD ссылка на сущность — тоже отказ, а не попытка что-то раскрыть.
    std::fs::write(&path, "<goods><item><name>&xxe;</name></item></goods>").unwrap();
    let error = read_xml(&path, &Limits::default()).unwrap_err().to_string();
    assert!(
        error.starts_with("сущность &xxe; не принимается"),
        "{error}"
    );
    assert!(!error.contains("МАРКЕР"));
}

#[test]
fn xml_depth_is_limited() {
    let base = Base::new("depth");
    let path = base.file("deep.xml");
    std::fs::write(&path, format!("{}{}", "<a>".repeat(40), "</a>".repeat(40))).unwrap();
    let limits = Limits {
        max_xml_depth: 2,
        ..Limits::default()
    };
    assert_eq!(
        limit_error(read_xml(&path, &limits)),
        "вложенность XML глубже 2"
    );
    let error = read_xml(&path, &Limits::default()).unwrap_err().to_string();
    assert_eq!(
        error, "запись 1: в поле вложенный элемент <a>",
        "глубже поля — не наш формат"
    );
}

#[test]
fn broken_files_give_errors() {
    let base = Base::new("broken");
    let noise_xlsx = base.file("noise.xlsx");
    std::fs::write(&noise_xlsx, noise(4096)).unwrap();
    let error = read_xlsx(&noise_xlsx, &Limits::default()).unwrap_err();
    assert!(
        matches!(error, IoError::File(ref reason) if reason.contains("не zip-архив")),
        "{error:?}"
    );

    // zip, но не xlsx.
    let not_xlsx = base.file("plain.xlsx");
    write_zip(&not_xlsx, &[("readme.txt", b"hello".to_vec())]);
    assert!(matches!(
        read_xlsx(&not_xlsx, &Limits::default()),
        Err(IoError::File(_))
    ));

    // Настоящий xlsx, обрезанный пополам.
    let cut = base.file("cut.xlsx");
    xlsx_with_rows(&cut, 50, "Мука");
    let bytes = std::fs::read(&cut).unwrap();
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    assert!(matches!(
        read_xlsx(&cut, &Limits::default()),
        Err(IoError::File(_))
    ));

    let garbage = base.file("garbage.xml");
    std::fs::write(&garbage, noise(512)).unwrap();
    assert!(read_xml(&garbage, &Limits::default()).is_err());
}

#[test]
fn honest_repetitive_sheet_is_not_a_bomb() {
    let base = Base::new("honest");
    let path = base.file("honest.xlsx");
    xlsx_with_rows(&path, 50_000, "Мука пшеничная высший сорт");
    let table = read_xlsx(&path, &Limits::default());
    assert_eq!(
        table.map(|t| t.rows.len()).map_err(|e| e.to_string()),
        Ok(50_000)
    );
}
