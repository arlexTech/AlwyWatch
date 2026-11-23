// SPDX-License-Identifier: AGPL-3.0-or-later
//! Compiles every `/i18n/<lang>.po` into static tables (no parsing at run time, nothing to load).
//! Only translated, non-fuzzy entries are kept; the rest falls back to English.

use std::fmt::Write as _;
use std::path::Path;

#[path = "src/po.rs"]
mod po;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../i18n");
    println!("cargo::rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "po"));
    files.sort();
    let mut out = String::from("pub static CATALOGS: &[Catalog] = &[\n");
    for path in files {
        println!("cargo::rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        let file = po::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let code = path.file_stem().unwrap().to_string_lossy().into_owned();
        let name = file.header("X-Language-Name").unwrap_or(&code).to_owned();
        let plural = file
            .header("Plural-Forms")
            .and_then(|p| p.split("plural=").nth(1))
            .map(|p| p.trim().trim_end_matches(';').to_owned())
            .unwrap_or_else(|| "n != 1".into());
        if let Err(e) = po::plural_index(&plural, 2) {
            panic!("{}: Plural-Forms: {e}", path.display());
        }
        let mut entries: Vec<_> = file
            .entries
            .into_iter()
            .filter(|e| !e.fuzzy && !e.msgid.is_empty() && e.msgstr.iter().all(|s| !s.is_empty()))
            .collect();
        entries.sort_by(|a, b| a.msgid.cmp(&b.msgid));
        entries.dedup_by(|a, b| a.msgid == b.msgid);
        writeln!(
            out,
            "    Catalog {{ code: {code:?}, name: {name:?}, plural: {plural:?}, entries: &["
        )
        .unwrap();
        for e in entries {
            writeln!(out, "        ({:?}, &{:?}),", e.msgid, e.msgstr).unwrap();
        }
        out.push_str("    ] },\n");
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("catalogs.rs");
    std::fs::write(dest, out).unwrap();
}
