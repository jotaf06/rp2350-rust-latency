//! Puts the chip's `memory/<chip>.x` on the linker search path as `memory.x`,
//! sets the linker arguments and exports the build-time facts used in the
//! META line of every batch: the cargo profile name and the git revision.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let rp2040 = env::var_os("CARGO_FEATURE_RP2040").is_some();
    let rp2350 = env::var_os("CARGO_FEATURE_RP2350").is_some();
    let chip = match (rp2350, rp2040) {
        (true, false) => "rp2350",
        (false, true) => "rp2040",
        _ => panic!(
            "enable exactly one chip feature: `rp2350` (default) or \
             `--no-default-features --features rp2040`"
        ),
    };
    let memory_x = format!("memory/{chip}.x");
    fs::copy(&memory_x, out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed={memory_x}");
    println!("cargo:rerun-if-changed=build.rs");

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    if rp2040 {
        // .boot2 section (second-stage bootloader), provided by embassy-rp.
        println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");
    }

    // Profile name. PROFILE is only ever "debug"/"release", so custom profiles
    // (release-z) are recovered from OUT_DIR = target/<triple>/<profile>/build/<pkg>/out.
    let profile = out
        .ancestors()
        .nth(3)
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| env::var("PROFILE").unwrap());
    println!("cargo:rustc-env=LAT_PROFILE={profile}");
    println!(
        "cargo:rustc-env=LAT_OPT_LEVEL={}",
        env::var("OPT_LEVEL").unwrap()
    );

    // Git revision ("unknown" outside a repository).
    let git = Command::new("git")
        .args(["describe", "--always", "--dirty", "--abbrev=8"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=LAT_GIT={git}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
