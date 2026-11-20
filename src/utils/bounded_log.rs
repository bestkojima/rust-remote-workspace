use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

/// The active path stays stable for log viewers. Keep the preceding segment
/// under a separate name; never scan or clean unrelated files in the directory.
pub(super) struct BoundedLog {
    path: PathBuf,
    backup: PathBuf,
    file: Option<File>,
    limit: usize,
    size: u64,
}

impl BoundedLog {
    pub fn open(path: &Path, limit: usize) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            backup: path.with_added_extension("1"),
            path: path.to_owned(),
            file: Some(file),
            size,
            limit,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.take();
        fs::rename(&self.path, &self.backup)?;
        self.size = 0;
        Ok(())
    }
}

impl Write for BoundedLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // Bound a single exceptionally large event as well as normal segments.
        let keep = buf.len().min(self.limit);
        if self.size > 0 && self.size.saturating_add(keep as u64) > self.limit as u64 {
            self.rotate()?;
        }
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        let file = self.file.as_mut().unwrap();
        file.write_all(&buf[..keep])?;
        self.size += keep as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_current_and_previous_segment_at_the_original_path() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("termirs.log");
        let mut w = BoundedLog::open(&base, 8).unwrap();
        for s in ["first\n", "second\n", "third\n"] {
            w.write_all(s.as_bytes()).unwrap();
        }
        assert_eq!(fs::read(&base).unwrap(), b"third\n");
        assert_eq!(
            fs::read(base.with_added_extension("1")).unwrap(),
            b"second\n"
        );
        w.write_all(&[b'x'; 100]).unwrap();
        assert_eq!(fs::metadata(&base).unwrap().len(), 8);
        assert_eq!(
            fs::read(base.with_added_extension("1")).unwrap(),
            b"third\n"
        );
    }
}
