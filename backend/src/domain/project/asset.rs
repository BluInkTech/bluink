// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) fn valid_filename(filename: &str) -> bool {
    let stem = filename
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let windows_device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    !filename.is_empty()
        && filename.len() <= 255
        && filename != "."
        && filename != ".."
        && !windows_device
        && !filename.ends_with('.')
        && !filename.ends_with(' ')
        && !filename.chars().any(|character| {
            matches!(
                character,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            ) || character.is_control()
        })
}

#[cfg(test)]
mod tests {

    use super::valid_filename;
    use crate::application::item_writes::write_asset;
    use crate::domain::project::config::create_project_config;

    #[test]
    fn confines_assets_to_the_owning_item() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let config = serde_json::json!({"format":"bluink-project","formatVersion":20,"id":uuid::Uuid::now_v7().to_string(),"defaultWorkspaceId":uuid::Uuid::now_v7().to_string(),"analytics":{"timezone":"UTC","eventRetentionDays":14}});
        create_project_config(&root, config).unwrap();
        let id = "01a0e2a0-0000-7000-8000-000000000001";
        crate::application::item_writes::write_item_input(&root,serde_json::json!({"id":id,"meta":{"type":"page","title":"Owner"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}),None,"2026-09-28").unwrap();
        let url = write_asset(&root, id, "cover.png", &[0, 1, 2]).unwrap();
        assert_eq!(url, format!("/assets/{id}/cover.png"));
        let portable = crate::store::assets::owner_path(id, "cover.png").unwrap();
        let store = crate::store::Store::shared(&root).unwrap();
        let mapping = store
            .read(|tx| crate::store::assets::mapping(tx, &portable))
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(mapping.available(&root).unwrap()).unwrap(),
            vec![0, 1, 2]
        );
        assert!(!root.join(&portable).exists());
        assert!(write_asset(&root, id, "../escape.png", &[]).is_err());
        assert!(write_asset(&root, id, "..\\escape.png", &[]).is_err());
        drop(store);
        temporary.close().unwrap();
    }

    #[test]
    fn rejects_reserved_and_cross_platform_unsafe_filenames() {
        for filename in [
            "", ".", "..", "a/b", "a\\b", "a:b", "a?b", "a.", "a ", "CON.png", "LPT1.txt",
        ] {
            assert!(!valid_filename(filename), "{filename}");
        }
        assert!(valid_filename("cover image.png"));
    }
}
