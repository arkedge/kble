use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let license_path = Path::new(&out_dir).join("LICENSE");
    let mut license_writer = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&license_path)
        .expect("failed to open LICENSE file");
    let mut child = std::process::Command::new("cargo")
        .args(&["about", "generate", "../about.hbs"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run cargo-about");
    let mut stdout = child.stdout.take().unwrap();
    std::io::copy(&mut stdout, &mut license_writer).expect("failed to write LICENSE file");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../Cargo.lock");
    println!("cargo:rerun-if-changed=../about.hbs");
}
