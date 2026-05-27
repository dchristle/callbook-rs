//! Generate the C header for the `cdylib`.
//!
//! Default behaviour: write `callbook.h` into `$OUT_DIR` so the build
//! never mutates anything under the source tree (works in vendored / read-only
//! / `cargo package` workflows). The committed copy at
//! `crates/callbook/include/callbook.h` is only refreshed when the
//! `CALLBOOK_REGEN_HEADER=1` environment variable is set, which is the
//! intended developer-time regeneration step before committing API changes.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=src/format.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CALLBOOK_REGEN_HEADER");

    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let crate_dir = PathBuf::from(crate_dir);
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let out_dir = PathBuf::from(out_dir);

    let cfg = match cbindgen::Config::from_file(crate_dir.join("cbindgen.toml")) {
        Ok(c) => c,
        Err(e) => {
            // Don't kill the build on header-gen failures — the rlib/cdylib
            // is still useful without an updated header.
            println!("cargo:warning=cbindgen config: {e}");
            return;
        }
    };

    let bindings = match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cfg)
        .generate()
    {
        Ok(b) => b,
        Err(e) => {
            let committed = crate_dir.join("include").join("callbook.h");
            let out_path = out_dir.join("callbook.h");
            if let Err(copy_err) = fs::copy(&committed, &out_path) {
                println!(
                    "cargo:warning=cbindgen failed: {e}; committed header fallback failed: {copy_err}"
                );
            } else if env::var_os("CALLBOOK_REGEN_HEADER").is_some() {
                println!("cargo:warning=cbindgen failed during requested header regeneration: {e}");
            }
            return;
        }
    };

    let out_path = out_dir.join("callbook.h");
    bindings.write_to_file(&out_path);

    // Make the OUT_DIR location discoverable by anything (e.g. release CI)
    // that wants to grab the freshly-generated header.
    println!("cargo:include={}", out_dir.display());
    println!("cargo:callbook_header={}", out_path.display());

    // Optional: refresh the committed copy. Only happens when the developer
    // explicitly opts in, so ordinary builds never dirty the source tree.
    if env::var_os("CALLBOOK_REGEN_HEADER").is_some() {
        let committed = crate_dir.join("include").join("callbook.h");
        bindings.write_to_file(&committed);
        println!(
            "cargo:warning=regenerated committed header at {}",
            committed.display()
        );
    }
}
