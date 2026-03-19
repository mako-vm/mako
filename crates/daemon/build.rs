use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let swift_src = manifest_dir
        .join("../../swift-ffi/Sources/VirtualizationFFI/MakoVM.swift")
        .canonicalize()
        .expect("Swift source not found");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed={}", swift_src.display());
    println!("cargo:rerun-if-changed=../../swift-ffi/include/mako_ffi.h");

    let sdk_path = get_macos_sdk_path();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "aarch64".into());
    let swift_target = match target_arch.as_str() {
        "aarch64" => "arm64-apple-macosx13.0",
        "x86_64" => "x86_64-apple-macosx13.0",
        other => panic!("Unsupported architecture: {other}"),
    };

    let obj_path = out_dir.join("mako_vz.o");
    let lib_path = out_dir.join("libmako_vz.a");

    // Compile Swift to object file
    let status = Command::new("swiftc")
        .args([
            "-c",
            "-parse-as-library",
            "-O",
            "-whole-module-optimization",
        ])
        .args(["-module-name", "MakoVirtualizationFFI"])
        .args(["-sdk", &sdk_path])
        .args(["-target", swift_target])
        .arg("-o")
        .arg(&obj_path)
        .arg(&swift_src)
        .status()
        .expect("Failed to run swiftc. Is Xcode Command Line Tools installed?");

    if !status.success() {
        panic!("swiftc compilation failed");
    }

    // Archive object file into a static library
    let status = Command::new("ar")
        .args(["rcs"])
        .arg(&lib_path)
        .arg(&obj_path)
        .status()
        .expect("Failed to run ar");

    if !status.success() {
        panic!("ar archive creation failed");
    }

    // Link the static library
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=mako_vz");

    // Link Apple frameworks
    println!("cargo:rustc-link-lib=framework=Virtualization");
    println!("cargo:rustc-link-lib=framework=vmnet");
    println!("cargo:rustc-link-lib=framework=Foundation");

    // Link Swift runtime (system-provided since macOS 12)
    let swift_lib_dir = get_swift_lib_dir();
    println!("cargo:rustc-link-search=native={swift_lib_dir}");

    // Link the Swift compatibility libraries
    let swift_compat_dir = get_swift_compat_dir();
    if Path::new(&swift_compat_dir).exists() {
        println!("cargo:rustc-link-search=native={swift_compat_dir}");
    }
}

fn get_macos_sdk_path() -> String {
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("Failed to run xcrun");
    String::from_utf8(output.stdout)
        .expect("Invalid UTF-8 from xcrun")
        .trim()
        .to_string()
}

fn get_swift_lib_dir() -> String {
    let output = Command::new("xcrun")
        .args(["--toolchain", "default", "--find", "swift"])
        .output()
        .expect("Failed to find swift");
    let swift_path = String::from_utf8(output.stdout)
        .expect("Invalid UTF-8")
        .trim()
        .to_string();
    // swift binary is at .../usr/bin/swift, we want .../usr/lib/swift/macosx
    let toolchain_dir = Path::new(&swift_path)
        .parent() // bin
        .and_then(|p| p.parent()) // usr
        .expect("Unexpected swift path layout");
    toolchain_dir
        .join("lib/swift/macosx")
        .to_string_lossy()
        .to_string()
}

fn get_swift_compat_dir() -> String {
    let output = Command::new("xcrun")
        .args(["--toolchain", "default", "--find", "swift"])
        .output()
        .expect("Failed to find swift");
    let swift_path = String::from_utf8(output.stdout)
        .expect("Invalid UTF-8")
        .trim()
        .to_string();
    let toolchain_dir = Path::new(&swift_path)
        .parent()
        .and_then(|p| p.parent())
        .expect("Unexpected swift path layout");
    toolchain_dir
        .join("lib/swift_static/macosx")
        .to_string_lossy()
        .to_string()
}
