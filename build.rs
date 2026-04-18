use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=macos/MediaRemoteHelper.swift");

    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("macos") {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR should be set"));
    let helper_path = out_dir.join("media_remote_helper");
    let status = Command::new("swiftc")
        .arg("macos/MediaRemoteHelper.swift")
        .arg("-O")
        .arg("-framework")
        .arg("Foundation")
        .arg("-framework")
        .arg("MediaPlayer")
        .arg("-o")
        .arg(&helper_path)
        .status()
        .expect("swiftc should be available");

    if !status.success() {
        panic!("failed to compile MediaRemoteHelper.swift");
    }

    println!(
        "cargo:rustc-env=SHUFFLE_MEDIA_HELPER={}",
        helper_path.display()
    );
}
