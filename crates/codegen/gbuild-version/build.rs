fn main() {
    println!("cargo:rerun-if-env-changed=GBUILD_VERSION");
}
