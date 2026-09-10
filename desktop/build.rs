use sha2::{Digest, Sha256};
fn main() {
    println!("cargo:rerun-if-changed=../target/release/cgagentharness");
    let backend = std::fs::read("../target/release/cgagentharness")
        .expect("build the release backend before building the desktop package");
    println!(
        "cargo:rustc-env=CGAH_BACKEND_SHA256={}",
        hex::encode(Sha256::digest(backend))
    );
    tauri_build::build();
}
