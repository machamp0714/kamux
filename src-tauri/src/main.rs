mod error;
mod model;
mod store;

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run kamux");
}
