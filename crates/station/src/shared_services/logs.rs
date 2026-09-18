//! Byte-oriented capture: long/binary lines cannot grow the Station's memory.
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
pub const LIMIT: u64 = 4 * 1024 * 1024;
pub const BACKUPS: usize = 3;

pub struct Writer {
    path: PathBuf,
    file: tokio::fs::File,
    size: u64,
    limit: u64,
}
impl Writer {
    pub async fn open(path: &Path) -> Result<Self> {
        tokio::fs::create_dir_all(path.parent().expect("log parent")).await?;
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let size = file.metadata().await?.len();
        Ok(Self {
            path: path.into(),
            file,
            size,
            limit: LIMIT,
        })
    }
    async fn rotate(&mut self) -> Result<()> {
        self.file.flush().await?;
        let numbered = |n| PathBuf::from(format!("{}.{}", self.path.display(), n));
        match tokio::fs::remove_file(numbered(BACKUPS)).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        for n in (1..BACKUPS).rev() {
            match tokio::fs::rename(numbered(n), numbered(n + 1)).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        tokio::fs::rename(&self.path, numbered(1)).await?;
        self.file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.path)
            .await?;
        self.size = 0;
        Ok(())
    }
    pub async fn capture(mut self, mut input: impl AsyncRead + Unpin) -> Result<()> {
        let mut bytes = [0; 8192];
        loop {
            let count = input.read(&mut bytes).await?;
            if count == 0 {
                self.file.flush().await?;
                return Ok(());
            }
            let mut remaining = &bytes[..count];
            while !remaining.is_empty() {
                if self.size >= self.limit {
                    self.rotate().await?;
                }
                let count = remaining.len().min((self.limit - self.size) as usize);
                self.file.write_all(&remaining[..count]).await?;
                self.size += count as u64;
                remaining = &remaining[count..];
            }
            self.file.flush().await?;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn failed_capture_reports_error_and_keeps_draining() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("stdout.log");
        let mut writer = Writer::open(&path).await?;
        writer.limit = 1;
        std::fs::remove_file(&path)?;
        let (mut output, input) = tokio::io::duplex(16);
        let (errors, observed) = tokio::sync::watch::channel(None);
        let capture = tokio::spawn(super::capture(writer, input, errors));
        // Rotation will fail after the first byte, but the producer still
        // finishes writing instead of receiving a broken pipe.
        tokio::time::timeout(Duration::from_secs(2), output.write_all(&vec![b'x'; 1024])).await??;
        drop(output);
        assert!(capture.await?.is_err());
        assert!(observed
            .borrow()
            .as_deref()
            .unwrap()
            .contains("service_log_capture_failed"));
        Ok(())
    }
    #[tokio::test]
    async fn binary_output_is_bounded_and_ordered_across_rotation() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("stdout.log");
        let mut writer = Writer::open(&path).await?;
        writer.limit = 128;
        let data = (0..700).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        writer.capture(data.as_slice()).await?;
        let mut saved = Vec::new();
        for n in (1..=BACKUPS).rev() {
            let bytes = std::fs::read(format!("{}.{}", path.display(), n))?;
            assert!(bytes.len() <= 128);
            saved.extend(bytes);
        }
        saved.extend(std::fs::read(&path)?);
        assert_eq!(saved, data[256..]);
        Ok(())
    }
}

/// Report a failed capture, then keep draining so logging failure does not
/// turn a server's stdout/stderr pipe into a broken pipe.
pub async fn capture(
    writer: Writer,
    mut input: impl AsyncRead + Unpin,
    errors: tokio::sync::watch::Sender<Option<String>>,
) -> Result<()> {
    let result = writer.capture(&mut input).await;
    if let Err(error) = &result {
        errors.send_replace(Some(format!("service_log_capture_failed: {error}")));
        let _ = tokio::io::copy(&mut input, &mut tokio::io::sink()).await;
    }
    result
}
