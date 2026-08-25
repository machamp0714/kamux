//! 契約 §29.8: `Cmd+W` 実装の前提条件として、既定メニューの Close Window を
//! 落としたことを固定する。`kamux::build_app_menu` を守る唯一の自動観測。
//!
//! # なぜ `harness = false` の統合テストなのか
//!
//! muda 0.19.3 はメニュー項目を構築するたびに実際の OS main thread
//! （`objc2::MainThreadMarker::new()`）を要求する
//! （`muda-0.19.3/src/platform_impl/macos/mod.rs:132`（`Menu::new`）/
//! `:328`（`Submenu::new`）ほか）。`cfg!(test)` によるマーカー省略は
//! `new_submenu` にしかなく、しかも muda 自身のクレート内テストにしか効かない
//! （依存クレート側からは `cfg!(test)` は常に false）。
//!
//! 通常の `#[test]`（libtest のデフォルトハーネス）は各テスト関数を
//! **ワーカースレッド**で実行するため、そこから `kamux::build_app_menu` を
//! 呼ぶと `` `muda::MenuChild` can only be created on the main thread ``
//! で panic する（実測。`src/lib.rs` から一度この形で書いて確認済み）。
//! `harness = false` を指定した統合テストバイナリの `fn main()` は
//! （libtest を経由しないため）プロセスの実の main thread で走るので、
//! ここでだけ `MockRuntime` 越しに実際のメニュー構築 API を呼んで列挙できる。
//!
//! `Cargo.toml` に `[[test]] name = "menu_no_close_window" harness = false`
//! が対になっている。

use tauri::menu::{Menu, MenuItemKind};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

/// トップレベルの `Submenu` ごとに、直下の `PredefinedMenuItem` の `text()` を
/// 集める（ネストした `Submenu` は無い前提。`kamux::build_app_menu` の構成が
/// そうなっている）。
fn submenu_item_texts(menu: &Menu<MockRuntime>) -> Vec<(String, Vec<String>)> {
    let mut result = Vec::new();
    for item in menu.items().expect("Menu::items") {
        if let MenuItemKind::Submenu(sub) = item {
            let title = sub.text().expect("Submenu::text");
            let children = sub.items().expect("Submenu::items");
            let child_texts: Vec<String> = children
                .iter()
                .filter_map(|c| match c {
                    MenuItemKind::Predefined(p) => {
                        Some(p.text().expect("PredefinedMenuItem::text"))
                    }
                    _ => None,
                })
                .collect();
            result.push((title, child_texts));
        }
    }
    result
}

fn main() {
    let app = mock_builder()
        .build(mock_context(noop_assets()))
        .expect("build mock app");

    let menu = kamux::build_app_menu(app.handle()).expect("build_app_menu");
    let submenus = submenu_item_texts(&menu);
    let all_texts: Vec<&String> = submenus.iter().flat_map(|(_, items)| items).collect();

    let mut failures: Vec<String> = Vec::new();

    // 1. Close Window がどのサブメニューにも存在しないこと（契約 §29.8）。
    if all_texts.iter().any(|t| t.as_str() == "Close Window") {
        failures.push(format!("Close Window が残っている: {submenus:?}"));
    }

    // 2. quit（Cmd+Q）が残っていること。`kill_on_run_event_exit` の doc コメントが
    //    依存する `terminate:` -> `RunEvent::Exit` の経路はこれが無いと発火しない。
    if !all_texts.iter().any(|t| t.starts_with("Quit")) {
        failures.push(format!("quit 項目が無い: {submenus:?}"));
    }

    // 3. Edit サブメニューの copy / paste が残っていること。WebView 上の
    //    Cmd+C / Cmd+V はこれらの既定項目のアクセラレータで動く。
    let edit_items = submenus
        .iter()
        .find(|(title, _)| title == "Edit")
        .map(|(_, items)| items.clone())
        .unwrap_or_default();
    if !edit_items.iter().any(|t| t == "Copy") {
        failures.push(format!("Edit に Copy が無い: {edit_items:?}"));
    }
    if !edit_items.iter().any(|t| t == "Paste") {
        failures.push(format!("Edit に Paste が無い: {edit_items:?}"));
    }

    // 4. Window サブメニューの中身をちょうど [Minimize, Zoom] に固定する。
    //    `all_texts` に対する「無いこと」だけの検査（1）は空メニューでも
    //    無条件に緑になる（空集合には何も含まれない）。ここで Window
    //    サブメニューの中身を具体的な値まで固定することで、「正しく削除された」
    //    ことと「そもそも何も無い」ことを弁別する（`Maximize` は macOS では
    //    "Zoom" と表示されることを実測済み。muda-0.19.3/src/items/predefined.rs:279）。
    let window_items = submenus
        .iter()
        .find(|(title, _)| title == "Window")
        .map(|(_, items)| items.clone());
    match window_items {
        Some(items) if items == vec!["Minimize".to_string(), "Zoom".to_string()] => {}
        other => failures.push(format!(
            "Window サブメニューの中身が想定と違う（Minimize, Zoom のみのはず）: {other:?}"
        )),
    }

    if failures.is_empty() {
        println!(
            "PASS: menu_no_close_window ({} 件のサブメニューを確認)",
            submenus.len()
        );
        std::process::exit(0);
    } else {
        for f in &failures {
            eprintln!("FAIL: {f}");
        }
        std::process::exit(1);
    }
}
