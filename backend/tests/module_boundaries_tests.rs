//! Архитектурная граница: модули `minecraft` и `launcher` не должны зависеть
//! от Cloud Storage. Общие операции живут в `storage`.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    files
}

#[test]
fn minecraft_and_launcher_do_not_depend_on_cloud() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for module in ["minecraft", "launcher"] {
        for file in rust_files(&src.join(module)) {
            let text = fs::read_to_string(&file).expect("исходник читается");
            checked += 1;
            assert!(
                !text.contains("crate::cloud"),
                "{} импортирует crate::cloud; перенесите общую операцию в storage",
                file.display()
            );
        }
    }
    assert!(checked > 0, "не найдено ни одного файла модулей");
}

#[test]
fn shared_storage_helpers_exist() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for relative in ["storage/http.rs", "storage/objects.rs", "storage/quota.rs"] {
        assert!(
            src.join(relative).is_file(),
            "ожидался общий модуль storage: {relative}"
        );
    }
}
