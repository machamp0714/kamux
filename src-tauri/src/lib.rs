// モジュールはすべて pub mod で宣言する。private mod にすると dead_code が消えない（§45.1 の実測）
pub mod error;
pub mod model;
pub mod store;

pub fn run() {
    tauri::Builder::default()
        // .manage(...) / .setup(...) / .invoke_handler(...) はすべてここに書く
        .run(tauri::generate_context!())
        .expect("failed to run kamux");
}
