fn main() {
    let id = std::env::var("LINDROIDS_BUILD_ID").unwrap_or_else(|_| "dev".into());
    println!("cargo:rustc-env=LINDROIDS_BUILD_ID={id}");
    println!("cargo:rerun-if-env-changed=LINDROIDS_BUILD_ID");
}
