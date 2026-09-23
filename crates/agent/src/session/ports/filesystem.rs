use std::io::{Read, Seek, Write};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
pub type FileFuture<T> = Pin<Box<dyn Future<Output = std::io::Result<T>> + Send>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePage {
    pub bytes: Vec<u8>,
    pub offset: u64,
    pub total_size: u64,
    pub next_offset: Option<u64>,
}

pub trait FileSystem: Send + Sync + 'static {
    fn list_page_async(
        self: Arc<Self>,
        _path: PathBuf,
        _cursor: Option<String>,
        _limit: usize,
    ) -> FileFuture<serde_json::Value> {
        Box::pin(async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Directory listing is unavailable",
            ))
        })
    }
    /// Network adapters override this with a cancellable async read. Ordinary
    /// filesystem implementations remain on the blocking executor.
    fn read_page_async(
        self: Arc<Self>,
        path: PathBuf,
        offset: u64,
        limit: usize,
    ) -> FileFuture<FilePage> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.read_page(&path, offset, limit))
                .await
                .map_err(std::io::Error::other)?
        })
    }

    fn create_dir_all(&self, path: &Path) -> std::io::Result<()>;

    fn create(&self, path: &Path) -> std::io::Result<Box<dyn Write + Send>>;

    fn read_page(&self, path: &Path, offset: u64, limit: usize) -> std::io::Result<FilePage>;

    fn read_to_string(&self, path: &Path) -> std::io::Result<String>;

    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()>;

    fn tail(&self, path: &Path, max_bytes: usize) -> std::io::Result<Vec<u8>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemFileSystem;

impl FileSystem for SystemFileSystem {
    fn list_page_async(
        self: Arc<Self>,
        path: PathBuf,
        cursor: Option<String>,
        limit: usize,
    ) -> FileFuture<serde_json::Value> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
            if !(1..=128).contains(&limit) { return Err(std::io::Error::other("invalid directory page size")); }
            let path=path.canonicalize()?;
            let modified=std::fs::metadata(&path)?.modified()?.duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other)?.as_nanos().to_string();
            let after=if let Some(cursor)=cursor {
                let (scope,version,after):(String,String,String)=serde_json::from_str(&cursor).map_err(std::io::Error::other)?;
                if scope!=path.to_string_lossy() || version!=modified {return Err(std::io::Error::other("directory changed; restart listing"));}
                Some(after)
            } else {None};
            let mut rows=std::collections::BTreeMap::new();
            for entry in std::fs::read_dir(&path)? {
                let entry=entry?;let name=entry.file_name().to_string_lossy().into_owned();
                if after.as_ref().is_some_and(|after|name<=*after) {continue;}
                let kind=entry.file_type()?;
                rows.insert(name,serde_json::json!({"path":entry.path(),"kind":if kind.is_dir(){"directory"}else if kind.is_symlink(){"symlink"}else{"file"},"size":entry.metadata()?.len()}));
                if rows.len()>limit+1 {rows.pop_last();}
            }
            if std::fs::metadata(&path)?.modified()?.duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other)?.as_nanos().to_string()!=modified {return Err(std::io::Error::other("directory changed; restart listing"));}
            let more=rows.len()>limit;
            let rows=rows.into_iter().take(limit).collect::<Vec<_>>();
            let cursor=if more {Some(serde_json::to_string(&(path.to_string_lossy(),modified,&rows.last().unwrap().0)).map_err(std::io::Error::other)?)} else {None};
            Ok(serde_json::json!({"path":path,"entries":rows.into_iter().map(|(_,entry)|entry).collect::<Vec<_>>(),"next_cursor":cursor}))
        }).await.map_err(std::io::Error::other)?
        })
    }
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn create(&self, path: &Path) -> std::io::Result<Box<dyn Write + Send>> {
        std::fs::File::create(path).map(|file| Box::new(file) as Box<dyn Write + Send>)
    }

    fn read_page(&self, path: &Path, offset: u64, limit: usize) -> std::io::Result<FilePage> {
        let mut file = std::fs::File::open(path)?;
        let total_size = file.metadata()?.len();
        let start = offset.min(total_size);
        file.seek(std::io::SeekFrom::Start(start))?;
        let length = total_size.saturating_sub(start).min(limit as u64) as usize;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)?;
        let end = start + length as u64;
        Ok(FilePage {
            bytes,
            offset,
            total_size,
            next_offset: (end < total_size).then_some(end),
        })
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, contents)
    }

    fn tail(&self, path: &Path, max_bytes: usize) -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(path)?;
        let size = file.seek(std::io::SeekFrom::End(0))?;
        let start = size.saturating_sub(max_bytes as u64);
        file.seek(std::io::SeekFrom::Start(start))?;
        let mut bytes = Vec::with_capacity((size - start) as usize);
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    #[tokio::test]
    async fn directory_pages_are_bounded_and_cursors_cannot_cross_roots_or_edits() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..140 {
            std::fs::write(root.path().join(format!("{i:03}.txt")), b"file").unwrap();
        }
        let files = Arc::new(SystemFileSystem);
        let first = files
            .clone()
            .list_page_async(root.path().into(), None, 100)
            .await
            .unwrap();
        assert_eq!(first["entries"].as_array().unwrap().len(), 100);
        let cursor = first["next_cursor"].as_str().unwrap().to_owned();
        let second = files
            .clone()
            .list_page_async(root.path().into(), Some(cursor.clone()), 100)
            .await
            .unwrap();
        assert_eq!(second["entries"].as_array().unwrap().len(), 40);
        let other = tempfile::tempdir().unwrap();
        assert!(files
            .clone()
            .list_page_async(other.path().into(), Some(cursor.clone()), 100)
            .await
            .is_err());
        std::fs::write(root.path().join("new.txt"), b"new").unwrap();
        assert!(files
            .list_page_async(root.path().into(), Some(cursor), 100)
            .await
            .is_err());
    }
}
