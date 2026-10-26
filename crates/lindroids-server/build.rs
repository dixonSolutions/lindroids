fn main() {
    // Release builds stamp the release version (0.1.<commits>) and the commit; dev builds say "dev".
    let version = std::env::var("LINDROIDS_VERSION").unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    let id = std::env::var("LINDROIDS_BUILD_ID").unwrap_or_else(|_| "dev".into());
    println!("cargo:rustc-env=LINDROIDS_VERSION={version}");
    println!("cargo:rustc-env=LINDROIDS_BUILD_ID={id}");
    println!("cargo:rerun-if-env-changed=LINDROIDS_VERSION");
    println!("cargo:rerun-if-env-changed=LINDROIDS_BUILD_ID");
}
