//! Normalize CA search paths for clients that treat SSL_CERT_DIR as a single directory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Only repair a multi-directory export. Explicit run --env options still take precedence.
pub fn env(runtime: &Path) -> Result<Vec<(String, String)>, String> {
    let Some(paths) = std::env::var_os("SSL_CERT_DIR") else { return Ok(Vec::new()) };
    if !paths.as_encoded_bytes().contains(&b':') {
        return Ok(Vec::new());
    }
    let dirs: Vec<_> = std::env::split_paths(&paths).filter(|p| !p.as_os_str().is_empty()).collect();
    let bundle = std::env::var_os("SSL_CERT_FILE").map(PathBuf::from);
    let dir = runtime.join(format!("ca-{}", std::process::id()));
    merge(&dir, &dirs, bundle.as_deref()).map_err(|e| format!("cannot merge SSL_CERT_DIR: {e}"))?;
    Ok(vec![("SSL_CERT_DIR".into(), dir.join("certs").display().to_string()), ("SSL_CERT_FILE".into(), dir.join("bundle.pem").display().to_string())])
}

fn pem_certificates(bytes: &[u8]) -> Vec<Vec<u8>> {
    let text = String::from_utf8_lossy(bytes);
    let mut rest = text.as_ref();
    let mut certs = Vec::new();
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    while let Some(i) = rest.find(BEGIN) {
        rest = &rest[i..];
        let Some(end) = rest.find(END) else { break };
        let end = end + END.len();
        certs.push(rest[..end].as_bytes().to_vec());
        rest = &rest[end..];
    }
    certs
}

fn merge(dest: &Path, sources: &[PathBuf], bundle: Option<&Path>) -> std::io::Result<()> {
    let mut files = BTreeMap::new();
    let mut certs = BTreeSet::new();
    if let Some(bytes) = bundle.and_then(|p| std::fs::read(p).ok()) {
        certs.extend(pem_certificates(&bytes));
    }
    for source in sources {
        let Ok(entries) = std::fs::read_dir(source) else { continue };
        // First source wins on basename collisions. Sort for reproducible bundles and links.
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if files.contains_key(&entry.file_name()) {
                continue;
            }
            let Ok(path) = std::fs::canonicalize(entry.path()) else { continue };
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let pem = pem_certificates(&bytes);
            if pem.is_empty() {
                continue;
            }
            certs.extend(pem);
            files.insert(entry.file_name(), path);
        }
    }
    if certs.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no readable PEM CA certificates in the source paths"));
    }
    std::fs::create_dir_all(dest.join("certs"))?;
    for (name, source) in files {
        let link = dest.join("certs").join(name);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(source, link)?;
    }
    let mut bytes = Vec::new();
    for cert in certs {
        bytes.extend(cert);
        bytes.push(b'\n');
    }
    std::fs::write(dest.join("bundle.pem"), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_hash_links_and_bundle_with_first_basename_winning() {
        let root = std::env::temp_dir().join(format!("lindroids-certs-{}", std::process::id()));
        let a = root.join("host");
        let b = root.join("android");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let pem = |s| format!("-----BEGIN CERTIFICATE-----\n{s}\n-----END CERTIFICATE-----\n");
        std::fs::write(a.join("same.0"), pem("host")).unwrap();
        std::fs::write(b.join("same.0"), pem("collision")).unwrap();
        std::fs::write(b.join("other.0"), format!("ignored text\n{}", pem("android"))).unwrap();
        std::os::unix::fs::symlink(a.join("same.0"), a.join("alias.pem")).unwrap();
        std::os::unix::fs::symlink(a.join("missing"), a.join("broken.0")).unwrap();
        std::fs::write(a.join("not-a-cert"), "no").unwrap();
        std::fs::write(root.join("extra.pem"), pem("explicit")).unwrap();
        let dest = root.join("merged");
        merge(&dest, &[a.clone(), b, root.join("missing")], Some(&root.join("extra.pem"))).unwrap();
        assert_eq!(std::fs::read_link(dest.join("certs/same.0")).unwrap(), a.join("same.0"));
        assert!(!dest.join("certs/broken.0").exists());
        assert!(!dest.join("certs/not-a-cert").exists());
        let bundle = std::fs::read_to_string(dest.join("bundle.pem")).unwrap();
        assert_eq!(bundle.matches("BEGIN CERTIFICATE").count(), 3);
        assert!(!bundle.contains("collision"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_sources_do_not_export_an_empty_trust_store() {
        let root = std::env::temp_dir().join(format!("lindroids-no-certs-{}", std::process::id()));
        assert!(merge(&root, &[], None).is_err());
        assert!(!root.exists());
    }
}
