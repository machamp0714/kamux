use crate::model::SurfaceKind;

/// イベントトピックに埋め込む決定的な ID（契約 §5）
pub fn surface_id(session_id: &str, kind: SurfaceKind) -> String {
    format!(
        "{}:{}",
        session_id,
        match kind {
            SurfaceKind::Agent => "agent",
            SurfaceKind::Editor => "editor",
        }
    )
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
