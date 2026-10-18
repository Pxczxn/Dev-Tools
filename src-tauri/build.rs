fn main() {
    let icon = std::path::Path::new("icons/icon.ico");
    if !icon.exists() {
        panic!(
            "icons/icon.ico not found.\n\
             Run `npm run icons` from the repository root \
             (generates Tauri icons from public/app-icon.svg).\n\
             Icons are also created automatically by `npm ci` via the postinstall script."
        );
    }

    tauri_build::build()
}
