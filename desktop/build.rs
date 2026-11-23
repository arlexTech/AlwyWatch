// SPDX-License-Identifier: AGPL-3.0-or-later
fn main() {
    // The shared catalogs (/i18n/<lang>.po) in the layout Slint bundles from:
    // <dir>/<lang>/LC_MESSAGES/<crate>.po. No translation context: one entry per English text,
    // shared with the other apps.
    let i18n = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../i18n");
    println!("cargo::rerun-if-changed={}", i18n.display());
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("translations");
    let _ = std::fs::remove_dir_all(&out);
    for entry in std::fs::read_dir(&i18n).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "po") {
            let lang = path.file_stem().unwrap().to_string_lossy().into_owned();
            let dir = out.join(&lang).join("LC_MESSAGES");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(&path, dir.join(format!("{}.po", env!("CARGO_PKG_NAME")))).unwrap();
        }
    }
    let mut config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    if out.exists() {
        config = config.with_bundled_translations(&out);
    }
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI compilation failed");
}
