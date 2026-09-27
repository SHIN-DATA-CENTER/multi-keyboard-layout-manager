//! Embeds VERSIONINFO and the application manifest (res/mklm-helper.rc) into mklm-helper.exe.
//! Same as apps/mklm-cli/build.rs apart from the file names (the manifest requires elevation).

use std::env;
use std::fs;
use std::path::PathBuf;

const RC_FILE: &str = "res/mklm-helper.rc";
const MANIFEST_FILE: &str = "res/mklm-helper.exe.manifest";
/// Generated into OUT_DIR, which embed-resource adds to the include path.
const VERSION_HEADER: &str = "mklm-helper-version.rch";

fn main() {
    println!("cargo:rerun-if-changed={RC_FILE}");
    println!("cargo:rerun-if-changed={MANIFEST_FILE}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    fs::write(out_dir.join(VERSION_HEADER), version_header())
        .expect("failed to write the version header");
    embed_resource::compile(RC_FILE, embed_resource::NONE)
        .manifest_required()
        .expect("failed to compile res/mklm-helper.rc");

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Plan 2.2: statically imported DLLs load from System32 only (LOAD_LIBRARY_SEARCH_SYSTEM32).
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
        // VCRUNTIME140.dll ships only with the VC++ Redistributable, and the flag above ignores an
        // app-local copy. Link the VC runtime statically and keep the Universal CRT, which is part of
        // Windows, dynamic ("hybrid CRT").
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

/// Version macros for the .rc, from `CARGO_PKG_VERSION*` (e.g. `0,1,0,0` and `"0.1.0"`).
fn version_header() -> String {
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
    // VS_FF_DEBUG for the dev and test profiles (PROFILE=debug).
    let flags = if env::var("PROFILE").as_deref() == Ok("debug") {
        "0x1L"
    } else {
        "0x0L"
    };
    format!(
        "#define MKLM_VERSION_NUM {major},{minor},{patch},0\n\
         #define MKLM_VERSION_STR \"{version}\"\n\
         #define MKLM_FILEFLAGS {flags}\n"
    )
}
