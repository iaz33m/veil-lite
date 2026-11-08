//! On Windows, embed an application manifest: Common Controls v6 (TaskDialog prompts), per-monitor-v2 DPI
//! awareness, long paths and UTF-8 code page.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest::embed_manifest(embed_manifest::new_manifest("Veil.Engine"))
            .expect("unable to embed the Windows manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
