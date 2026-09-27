//! Builds the GUI (design m3 C, D, F.7):
//!
//! - compiles `ui/app.slint` with the fluent style and the bundled translations of
//!   `translations/<lang>/LC_MESSAGES/mklm.po` (no default translation context, so the .po files
//!   carry plain msgids; extract with `slint-tr-extractor --no-default-translation-context`);
//! - embeds VERSIONINFO and the application manifest (res/mklm.rc: asInvoker, PerMonitorV2,
//!   UTF-8), and the build ID (design m2 E.3) both as `env!("MKLM_BUILD_ID")` and as the
//!   VERSIONINFO string `MKLMBuildId`, computed by apps/build_id.rs exactly as the CLI and the
//!   helper do: the GUI launches the helper, which accepts only its own build;
//! - links like the other executables: `/DEPENDENTLOADFLAG:0x800` and the hybrid CRT.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "../build_id.rs"]
mod build_id;

const RC_FILE: &str = "res/mklm.rc";
const MANIFEST_FILE: &str = "res/mklm.exe.manifest";
/// Generated into OUT_DIR, which embed-resource adds to the include path.
const VERSION_HEADER: &str = "mklm-version.rch";

fn main() {
    println!("cargo:rerun-if-changed={RC_FILE}");
    println!("cargo:rerun-if-changed={MANIFEST_FILE}");
    println!("cargo:rerun-if-changed=translations");

    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config)
        .expect("failed to compile ui/app.slint");

    // On every target, so that `env!("MKLM_BUILD_ID")` always compiles.
    let build_id = embed_build_id();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    fs::write(out_dir.join(VERSION_HEADER), version_header(&build_id))
        .expect("failed to write the version header");
    embed_resource::compile(RC_FILE, embed_resource::NONE)
        .manifest_required()
        .expect("failed to compile res/mklm.rc");

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Plan 2.2: statically imported DLLs load from System32 only (LOAD_LIBRARY_SEARCH_SYSTEM32).
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
        // As the CLI and the helper: the VC runtime statically, the Universal CRT dynamically.
        for arg in [
            "/NODEFAULTLIB:msvcrt.lib",
            "/NODEFAULTLIB:vcruntime.lib",
            "/NODEFAULTLIB:libucrt.lib",
            "/DEFAULTLIB:libcmt.lib",
            "/DEFAULTLIB:libvcruntime.lib",
            "/DEFAULTLIB:ucrt.lib",
        ] {
            println!("cargo:rustc-link-arg-bins={arg}");
        }
    }
}

/// Computes the build ID, passes it to rustc as `MKLM_BUILD_ID` and returns it.
fn embed_build_id() -> String {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("the app lives two levels below the workspace root");
    println!("cargo:rerun-if-changed=../build_id.rs");
    for path in build_id::watched_paths(workspace_root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by Cargo");
    let id =
        build_id::build_id(workspace_root, &version).expect("failed to hash the shared crates");
    println!("cargo:rustc-env=MKLM_BUILD_ID={id}");
    id
}

/// Version macros for the .rc (see apps/mklm-cli/build.rs).
fn version_header(build_id: &str) -> String {
    let part = |name: &str| -> u16 {
        env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{name} does not fit a VERSIONINFO field"))
    };
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by Cargo");
    let flags = if env::var("PROFILE").as_deref() == Ok("debug") {
        "0x1L"
    } else {
        "0x0L"
    };
    format!(
        "#define MKLM_VERSION_NUM {major},{minor},{patch},0\n\
         #define MKLM_VERSION_STR \"{version}\"\n\
         #define MKLM_FILEFLAGS {flags}\n\
         #define MKLM_BUILD_ID_STR \"{build_id}\"\n"
    )
}
