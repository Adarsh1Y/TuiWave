//! Crash-safe file writes: write to a temp file, then atomically rename.

use std::io;
use std::io::Write;
use std::path::Path;

/// Write `data` to `path` atomically (temp file + rename on the same
/// filesystem). A crash/power-loss mid-write leaves the previous file intact
/// instead of a truncated one.
pub fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp_path = Path::new(&tmp);

    {
        let mut f = std::fs::File::create(tmp_path)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    match std::fs::rename(tmp_path, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(tmp_path);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lw-atomic-{}", std::process::id()));
        let path = dir.join("x");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        atomic_write(&path, b"world").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "world");
        std::fs::remove_dir_all(&dir).ok();
    }
}