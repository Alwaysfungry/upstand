use std::fs;
use std::path::Path;

fn main() {
    // Keep reminder header icon in sync with icon assets.
    let src_small = Path::new("icons/icon-32.png");
    let src_fallback = Path::new("icons/icon.png");
    let dst = Path::new("dist/reminder-icon.png");
    if src_small.exists() {
        let _ = fs::copy(src_small, dst);
    } else if src_fallback.exists() {
        let _ = fs::copy(src_fallback, dst);
    }
    tauri_build::build();
}
