//! Builds one program's app on a computer, for testing:
//! `cargo run -p lindroids-apk --example build-app -- <out.apk> <id> <name> <start> <launch> <icon.png> [key file]`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 6 {
        eprintln!("usage: build-app <out.apk> <id> <name> <start> <launch> <icon.png> [key file]");
        std::process::exit(2);
    }
    let key_path = a.get(6).cloned().unwrap_or_else(|| "target/lindroids-apk-test.key".into());
    let key = match std::fs::read(&key_path) {
        Ok(b) => lindroids_apk::Key::from_bytes(&b).unwrap(),
        Err(_) => {
            let k = lindroids_apk::Key::generate("Lindroids test").unwrap();
            std::fs::write(&key_path, k.to_bytes()).unwrap();
            k
        }
    };
    let icon = std::fs::read(&a[5]).unwrap();
    let package = lindroids_apk::package_for(&a[1]);
    let p = lindroids_apk::Program {
        package: &package,
        name: &a[2],
        start: &a[3],
        launch: &a[4],
        termux: "com.termux",
        id: &a[1],
        version_code: (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() / 60) as u32,
        version_name: "0.1.0",
        icon: &icon,
        foreground: &icon,
        background: &icon,
    };
    let t = std::time::Instant::now();
    let apk = lindroids_apk::build(&p, &key).unwrap();
    std::fs::write(&a[0], &apk).unwrap();
    eprintln!("{} {} bytes, {} in {:?}, signer {}", a[0], apk.len(), package, t.elapsed(), key.fingerprint());
}
