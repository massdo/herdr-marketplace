use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::application::ports::{CachedIndex, CatalogCache};

const FILE: &str = "index";

/// One file: the URL and the ETag on a line each, then the index as
/// downloaded. Without a directory, nothing is saved.
pub struct FileCatalogCache {
    dir: Option<PathBuf>,
}

impl FileCatalogCache {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }
}

impl CatalogCache for FileCatalogCache {
    fn read(&self) -> Option<CachedIndex> {
        let bytes = fs::read(self.dir.as_ref()?.join(FILE)).ok()?;
        let mut parts = bytes.splitn(3, |&byte| byte == b'\n');
        let url = String::from_utf8(parts.next()?.to_vec()).ok()?;
        let etag = String::from_utf8(parts.next()?.to_vec()).ok()?;
        let body = parts.next()?.to_vec();
        Some(CachedIndex { url, etag, body })
    }

    fn replace(&self, entry: &CachedIndex) {
        if let Some(dir) = &self.dir {
            let _ = write(dir, entry);
        }
    }

    fn remove(&self) {
        if let Some(dir) = &self.dir {
            let _ = fs::remove_file(dir.join(FILE));
        }
    }
}

/// Written to a file of its own, then renamed: URL, ETag and index change
/// together, and two sidebars saving at once leave one whole file.
fn write(dir: &Path, entry: &CachedIndex) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    write!(file, "{}\n{}\n", entry.url, entry.etag)?;
    file.write_all(&entry.body)?;
    file.persist(dir.join(FILE))?;
    Ok(())
}
