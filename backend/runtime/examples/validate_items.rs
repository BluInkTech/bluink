//! Development audit for migrating an existing local project to the Rust engine.
//! Run with: cargo run --example `validate_items` -- <project-root>

// Example targets inherit engine dependencies but intentionally use only its API.
#![allow(unused_crate_dependencies)]
#![allow(clippy::redundant_pub_crate)]

// Include the private engine for this standalone development audit.
// Product internals are not made public solely to support an example binary.
#[path = "../src/engine/mod.rs"]
#[allow(dead_code)]
mod engine;
#[path = "../src/parallel.rs"]
mod parallel;

use std::env;
use std::fs;
use std::path::Path;

use engine::project::item::read_item;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args().nth(1).ok_or("Expected a project root")?;
    let mut checked = 0usize;
    let mut failed = 0usize;
    for prefix in fs::read_dir(Path::new(&root).join("items"))? {
        let prefix = prefix?;
        if !prefix.file_type()?.is_dir() {
            continue;
        }
        for directory in fs::read_dir(prefix.path())? {
            let directory = directory?;
            if !directory.file_type()?.is_dir() {
                continue;
            }
            let id = directory.file_name();
            let Some(id) = id.to_str() else { continue };
            let path = directory.path().join("item.json");
            if !path.is_file() {
                continue;
            }
            checked += 1;
            if checked.is_multiple_of(10_000) {
                eprintln!("Checked {checked} items...");
            }
            if let Err(error) = read_item(&path, id) {
                failed += 1;
                if failed <= 20 {
                    eprintln!("{error}");
                }
            }
        }
    }
    eprintln!("Validated {checked} items; {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
