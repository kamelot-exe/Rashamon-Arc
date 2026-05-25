use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=webext/rashamon_webext.c");
    if env::var_os("CARGO_FEATURE_WEBKIT").is_none() {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap_or_default());
    let source = manifest_dir.join("webext").join("rashamon_webext.c");
    let output = out_dir.join("librashamon_webext.so");

    let cflags = run_capture(["--cflags", "webkit2gtk-web-extension-4.1"]);
    let libs = run_capture(["--libs", "webkit2gtk-web-extension-4.1"]);
    let (Some(cflags), Some(libs)) = (cflags, libs) else {
        println!("cargo:warning=webkit web extension disabled: pkg-config webkit2gtk-web-extension-4.1 not found");
        return;
    };

    let mut cmd = Command::new("cc");
    cmd.arg("-shared")
        .arg("-fPIC")
        .arg("-O2")
        .arg("-o")
        .arg(&output)
        .arg(&source);
    for flag in cflags.split_whitespace() {
        cmd.arg(flag);
    }
    for flag in libs.split_whitespace() {
        cmd.arg(flag);
    }

    match cmd.status() {
        Ok(status) if status.success() => {
            println!("cargo:rustc-env=RASHAMON_WEBEXT_DIR={}", out_dir.display());
        }
        Ok(status) => {
            println!(
                "cargo:warning=webkit web extension build failed (status: {status}), falling back to in-process adblock"
            );
        }
        Err(err) => {
            println!(
                "cargo:warning=webkit web extension build failed ({err}), falling back to in-process adblock"
            );
        }
    }
}

fn run_capture<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("pkg-config").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
