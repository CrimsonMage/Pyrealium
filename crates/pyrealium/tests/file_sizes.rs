//! The file size rule (README, Build and verify): no `.rs` file in the workspace reaches 1500 lines.
use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != ".git" { sources(&path, out); }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_rust_file_reaches_1500_lines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    sources(&root.join("crates"), &mut files);
    let over: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let lines = std::fs::read_to_string(f).ok()?.lines().count();
            (lines >= 1500).then(|| format!("{lines} {}", f.display()))
        })
        .collect();
    assert!(over.is_empty(), "split these files by responsibility (README file size rule): {over:#?}");
}
