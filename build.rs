use std::env::var;

fn main() {
    // The manifest dir points to the root of the project containing this file.
    let manifest_dir = var("CARGO_MANIFEST_DIR").unwrap();
    // We tell Cargo that our native ARMv7 libraries are inside a "libraries" folder.
    println!("cargo:rustc-link-search={}/libraries/lib/arm-linux-gnueabihf", manifest_dir);
    println!("cargo:rustc-link-search={}/libraries/usr/lib/arm-linux-gnueabihf", manifest_dir);
}
