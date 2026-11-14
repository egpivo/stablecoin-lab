//! SHA-256 manifest for raw source files (`<hex>  <relative path>`, sha256sum format).

use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

pub fn digest(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

pub fn write(raw_dir: &Path, files: &[&str], manifest: &Path) -> anyhow::Result<()> {
    let mut out = String::new();
    for f in files {
        out.push_str(&format!("{}  {}\n", digest(&raw_dir.join(f))?, f));
    }
    if let Some(parent) = manifest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(manifest, out)?;
    Ok(())
}

/// Every listed file exists and matches its digest.
pub fn verify(raw_dir: &Path, manifest: &Path) -> anyhow::Result<usize> {
    let text =
        fs::read_to_string(manifest).with_context(|| format!("read {}", manifest.display()))?;
    let mut n = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (want, file) = line
            .split_once("  ")
            .with_context(|| format!("bad manifest line: {line}"))?;
        let got = digest(&raw_dir.join(file))?;
        if got != want {
            bail!("{file}: sha256 {got} does not match manifest {want}");
        }
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::write(dir.path().join("a/x.csv"), "1,2\n").unwrap();
        let m = dir.path().join("m/raw.sha256");
        write(dir.path(), &["a/x.csv"], &m).unwrap();
        assert_eq!(verify(dir.path(), &m).unwrap(), 1);
        fs::write(dir.path().join("a/x.csv"), "1,3\n").unwrap();
        assert!(verify(dir.path(), &m).is_err());
    }
}
