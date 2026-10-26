// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Opt-in, read-only audit using the production reader without duplicating the
//! private runtime module tree in a standalone binary.

#[test]
#[ignore = "Set BLUINK_AUDIT_PROJECT to an existing project to audit its records"]
fn validate_items() {
    let root = std::path::PathBuf::from(
        std::env::var_os("BLUINK_AUDIT_PROJECT").expect("Set BLUINK_AUDIT_PROJECT"),
    );
    let ids = super::item::list_project_item_ids(&root).unwrap();
    let mut failed = 0;
    for id in &ids {
        if let Err(error) = super::item::read_project_item(&root, id) {
            failed += 1;
            if failed <= 20 {
                eprintln!("{error}");
            }
        }
    }
    eprintln!("Validated {} items; {failed} failed", ids.len());
    assert_eq!(failed, 0, "Project validation failed");
}
