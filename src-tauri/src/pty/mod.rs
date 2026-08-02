use crate::model::SurfaceKind;

pub mod backpressure;
pub mod surface;

// 契約 §15 は `crate::pty::SpawnSpec` というパスを前提にする。実体は surface.rs に
// あるため、ここで再エクスポートしてそのパス解決を成立させる。
pub use surface::SpawnSpec;

/// イベントトピックに埋め込む決定的な ID（契約 §5）。
/// 文字列表現は `SurfaceKind::as_db_str`（model.rs の `db_enum!`）を単一の情報源として使う。
pub fn surface_id(session_id: &str, kind: SurfaceKind) -> String {
    format!("{}:{}", session_id, kind.as_db_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_id_joins_session_and_kind_with_colon() {
        assert_eq!(
            surface_id("3f2a9c1e-0000-4000-8000-000000000001", SurfaceKind::Agent),
            "3f2a9c1e-0000-4000-8000-000000000001:agent"
        );
        assert_eq!(
            surface_id("3f2a9c1e-0000-4000-8000-000000000001", SurfaceKind::Editor),
            "3f2a9c1e-0000-4000-8000-000000000001:editor"
        );
    }
}
