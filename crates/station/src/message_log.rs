//! Immutable Chat records. SQLite here indexes byte locations only and can be
//! removed and rebuilt from the JSONL / independent zstd frames.
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};
use zork_client_types::{chat::Message, pages::DeliveredPage};

const SEGMENT_BYTES: u64 = 32 * 1024 * 1024;
const FRAME_BYTES: usize = 256 * 1024;
const DECODE_CACHE_BYTES: usize = 4 * 1024 * 1024;
// Old nodes accepted larger visible records; do not silently truncate them.
const MAX_RECORD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub sequence: i64,
    pub session_key: String,
    pub role: String,
    pub kind: Option<String>,
    pub message: Message,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<DeliveredPage>,
}
impl Record {
    pub fn text(&self) -> String {
        zork_client_types::files::compose(&self.message.text, &self.message.attachments)
    }
}

pub(crate) struct MessageLog {
    root: PathBuf,
    state: Mutex<Index>,
    epochs: Mutex<HashMap<String, String>>,
    _writer: File,
    segment_bytes: u64,
}
struct Index {
    conn: Connection,
    frames: HashMap<(String, u64), Arc<Vec<u8>>>,
    recent: VecDeque<(String, u64)>,
    bytes: usize,
    dirty: bool,
}
#[derive(Clone)]
struct Location {
    path: String,
    compressed: bool,
    frame: u64,
    frame_len: u64,
    raw_len: u64,
    offset: u64,
    length: u64,
}

impl MessageLog {
    pub fn open(root: &Path, cache: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        let writer = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".writer.lock"))?;
        writer
            .try_lock()
            .context("Chat message source already has a writer")?;
        fs::create_dir_all(cache)?;
        let index_path = cache.join("chat-message-locations.sqlite");
        let open_index = || -> Result<Connection> {
            let conn = Connection::open(&index_path)?;
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY,chat TEXT NOT NULL,bytes INTEGER NOT NULL,modified TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS records(sequence INTEGER PRIMARY KEY,chat TEXT NOT NULL,id TEXT NOT NULL,path TEXT NOT NULL,
                compressed INTEGER NOT NULL,frame INTEGER NOT NULL,frame_len INTEGER NOT NULL,raw_len INTEGER NOT NULL,
                offset INTEGER NOT NULL,length INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS record_chat ON records(chat,sequence);
            CREATE INDEX IF NOT EXISTS record_id ON records(chat,id,sequence);
            CREATE INDEX IF NOT EXISTS record_file ON records(path);")?;
            let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            ensure!(check == "ok", "invalid message location cache: {check}");
            Ok(conn)
        };
        let conn = match open_index() {
            Ok(conn) => conn,
            Err(error) if index_path.is_file() => {
                // Only the disposable location cache is replaced. Source data
                // is never repaired or discarded because an index is damaged.
                let suffix = format!("unusable-{}", ulid::Ulid::new());
                for ending in ["", "-wal", "-shm"] {
                    let path = cache.join(format!("chat-message-locations.sqlite{ending}"));
                    if path.exists() {
                        fs::rename(
                            &path,
                            cache.join(format!("chat-message-locations.sqlite.{suffix}{ending}")),
                        )?;
                    }
                }
                tracing::warn!(%error,"Rebuilding Chat message location cache");
                open_index()?
            }
            Err(error) => return Err(error),
        };
        let log = Self {
            root: root.into(),
            state: Mutex::new(Index {
                conn,
                frames: HashMap::new(),
                recent: VecDeque::new(),
                bytes: 0,
                dirty: false,
            }),
            _writer: writer,
            segment_bytes: SEGMENT_BYTES,
            epochs: Mutex::new(HashMap::new()),
        };
        log.refresh(&mut log.state.lock().unwrap())?;
        Ok(log)
    }

    pub fn ensure_chat(&self, chat: &str) -> Result<()> {
        let directory = self.root.join(component(chat)?);
        let metadata = directory.join(".zork");
        let path = metadata.join("source.json");
        if path.is_file() {
            return Ok(());
        }
        fs::create_dir_all(&metadata)?;
        fs::create_dir_all(directory.join("messages"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(&metadata)?;
        serde_json::to_writer(
            &mut temporary,
            &serde_json::json!({"format":1,"chat_id":chat,"epoch":ulid::Ulid::new().to_string()}),
        )?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
        sync_directory(&metadata)?;
        sync_directory(&directory)?;
        sync_directory(&self.root)?;
        Ok(())
    }

    pub fn epoch(&self, chat: &str) -> Result<String> {
        if let Some(epoch) = self.epochs.lock().unwrap().get(chat) {
            return Ok(epoch.clone());
        }
        self.ensure_chat(chat)?;
        let value: serde_json::Value = serde_json::from_reader(File::open(
            self.root.join(component(chat)?).join(".zork/source.json"),
        )?)?;
        ensure!(
            value["format"] == 1 && value["chat_id"] == chat,
            "invalid Chat source metadata"
        );
        let epoch = value["epoch"]
            .as_str()
            .context("Chat source epoch missing")?;
        component(epoch)?;
        self.epochs
            .lock()
            .unwrap()
            .insert(chat.into(), epoch.into());
        Ok(epoch.into())
    }

    /// The caller supplies committed source positions. Recovery only checks
    /// an already-written physical position; it never searches message IDs to
    /// combine separate sends.
    pub fn append(&self, records: &[Record]) -> Result<()> {
        let mut index = self.state.lock().unwrap();
        if index.dirty {
            self.refresh(&mut index)?;
            index.dirty = false;
        }
        let mut touched = std::collections::HashSet::new();
        for record in records {
            ensure!(record.sequence > 0, "invalid message source position");
            let chat = &record.message.chat_id;
            let folder = component(chat)?;
            self.ensure_chat(folder)?;
            if let Some(location) = location(&index.conn, record.sequence)? {
                let saved = self.read_at(&mut index, &location)?;
                ensure!(
                    serde_json::to_value(&saved)? == serde_json::to_value(record)?,
                    "message source position changed"
                );
                // A prior append may have failed while syncing its batch. The
                // caller still has a staged publication, so sync this prefix
                // again before allowing it to clear that durable staging row.
                if !location.compressed {
                    touched.insert(location.path);
                }
                continue;
            }
            let tail: Option<(i64, String)> = index.conn.query_row(
                "SELECT sequence,path FROM records WHERE chat=?1 ORDER BY sequence DESC LIMIT 1",
                [chat], |row|Ok((row.get(0)?,row.get(1)?)),
            ).optional()?;
            ensure!(
                tail.as_ref()
                    .is_none_or(|(last, _)| record.sequence > *last),
                "message source moved backwards"
            );
            let mut bytes = serde_json::to_vec(record)?;
            bytes.push(b'\n');
            ensure!(
                bytes.len() as u64 <= MAX_RECORD_BYTES,
                "message source record too large"
            );
            index.dirty = true;
            let mut path = tail
                .as_ref()
                .filter(|(_, path)| path.ends_with(".jsonl"))
                .map(|(_, path)| path.clone());
            if let Some(active) = &path {
                let len = fs::metadata(self.root.join(active))?.len();
                if len > 0 && len.saturating_add(bytes.len() as u64) > self.segment_bytes {
                    self.seal(&mut index, active)?;
                    touched.remove(active);
                    path = None;
                }
            }
            let path =
                path.unwrap_or_else(|| format!("{folder}/messages/{:020}.jsonl", record.sequence));
            let full = self.root.join(&path);
            let parent = full.parent().context("message segment directory")?;
            fs::create_dir_all(parent)?;
            let new = !full.exists();
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .read(true)
                .open(&full)?;
            let start = file.metadata()?.len();
            if let Err(error) = file.write_all(&bytes) {
                file.set_len(start)
                    .context("restore incomplete message append")?;
                file.sync_all()?;
                return Err(error.into());
            }
            if new {
                sync_directory(parent)?;
                sync_directory(parent.parent().unwrap())?;
                sync_directory(&self.root)?;
            }
            let tx = index.conn.unchecked_transaction()?;
            insert_location(
                &tx,
                record,
                &Location {
                    path: path.clone(),
                    compressed: false,
                    frame: 0,
                    frame_len: 0,
                    raw_len: 0,
                    offset: start,
                    length: bytes.len() as u64,
                },
            )?;
            register_file(&tx, &path, chat, &full)?;
            tx.commit()?;
            touched.insert(path);
        }
        for path in touched {
            File::open(self.root.join(path))?.sync_all()?;
        }
        index.dirty = false;
        Ok(())
    }

    pub fn get(&self, sequence: i64) -> Result<Record> {
        self.get_optional(sequence)?
            .context("message source record missing")
    }

    pub fn get_optional(&self, sequence: i64) -> Result<Option<Record>> {
        let mut index = self.state.lock().unwrap();
        if index.dirty {
            self.refresh(&mut index)?;
            index.dirty = false;
        }
        let Some(location) = location(&index.conn, sequence)? else {
            return Ok(None);
        };
        let record = self.read_at(&mut index, &location)?;
        ensure!(record.sequence == sequence, "message source index mismatch");
        Ok(Some(record))
    }

    pub fn positions(&self, after: i64, limit: usize) -> Result<Vec<i64>> {
        let index = self.state.lock().unwrap();
        let rows = index
            .conn
            .prepare("SELECT sequence FROM records WHERE sequence>?1 ORDER BY sequence LIMIT ?2")?
            .query_map(params![after, limit], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn read_at(&self, index: &mut Index, location: &Location) -> Result<Record> {
        ensure!(
            location.length <= MAX_RECORD_BYTES,
            "invalid message index length"
        );
        let raw = if location.compressed {
            let key = (location.path.clone(), location.frame);
            let frame = if let Some(frame) = index.frames.get(&key) {
                frame.clone()
            } else {
                ensure!(
                    location.raw_len <= self.segment_bytes.max(MAX_RECORD_BYTES),
                    "invalid message frame length"
                );
                let mut file = File::open(self.root.join(&location.path))?;
                file.seek(SeekFrom::Start(location.frame))?;
                let mut decoder = zstd::stream::read::Decoder::new(file.take(location.frame_len))?;
                let mut raw = Vec::new();
                decoder
                    .by_ref()
                    .take(location.raw_len.saturating_add(1))
                    .read_to_end(&mut raw)?;
                ensure!(
                    raw.len() as u64 == location.raw_len,
                    "message frame length mismatch"
                );
                let frame = Arc::new(raw);
                if frame.len() <= DECODE_CACHE_BYTES {
                    while index.bytes + frame.len() > DECODE_CACHE_BYTES {
                        if let Some(old) = index.recent.pop_front() {
                            if let Some(value) = index.frames.remove(&old) {
                                index.bytes -= value.len();
                            }
                        }
                    }
                    index.bytes += frame.len();
                    index.recent.push_back(key.clone());
                    index.frames.insert(key, frame.clone());
                }
                frame
            };
            let end = location
                .offset
                .checked_add(location.length)
                .context("message index overflow")?;
            ensure!(end <= frame.len() as u64, "message index outside frame");
            frame[location.offset as usize..end as usize].to_vec()
        } else {
            let mut file = File::open(self.root.join(&location.path))?;
            file.seek(SeekFrom::Start(location.offset))?;
            let mut raw = vec![0; location.length as usize];
            file.read_exact(&mut raw)?;
            raw
        };
        Ok(serde_json::from_slice(&raw)?)
    }

    fn refresh(&self, index: &mut Index) -> Result<()> {
        let mut files = BTreeMap::new();
        for chat in fs::read_dir(&self.root)? {
            let chat = chat?;
            if !chat.file_type()?.is_dir() {
                continue;
            }
            let folder = chat.file_name().to_string_lossy().into_owned();
            component(&folder)?;
            let directory = chat.path().join("messages");
            if !directory.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let stem = name
                    .strip_suffix(".jsonl.zst")
                    .or_else(|| name.strip_suffix(".jsonl"));
                let Some(stem) = stem else {
                    continue;
                };
                let first = stem
                    .parse::<i64>()
                    .context("invalid message segment name")?;
                let key = (folder.clone(), first);
                let path = format!("{folder}/messages/{name}");
                // Published .zst files are complete and verified before rename.
                if name.ends_with(".zst") || !files.contains_key(&key) {
                    files.insert(key, path);
                }
            }
        }
        let selected: std::collections::HashSet<_> = files.values().cloned().collect();
        let old = index
            .conn
            .prepare("SELECT path FROM files")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for path in old {
            if !selected.contains(&path) {
                index
                    .conn
                    .execute("DELETE FROM records WHERE path=?1", [&path])?;
                index
                    .conn
                    .execute("DELETE FROM files WHERE path=?1", [&path])?;
            }
        }
        for (_, path) in files {
            let full = self.root.join(&path);
            let (bytes, modified) = stamp(&full)?;
            let saved: Option<(u64, String)> = index
                .conn
                .query_row(
                    "SELECT bytes,modified FROM files WHERE path=?1",
                    [&path],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if saved == Some((bytes, modified)) {
                continue;
            }
            self.index_file(index, &path)?;
        }
        Ok(())
    }

    fn index_file(&self, index: &mut Index, path: &str) -> Result<()> {
        let full = self.root.join(path);
        index.frames.clear();
        index.recent.clear();
        index.bytes = 0;
        let tx = index.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM records WHERE path=?1", [path])?;
        let mut chat = None;
        let mut previous = 0;
        if path.ends_with(".zst") {
            let mut reader = BufReader::new(File::open(&full)?);
            while !reader.fill_buf()?.is_empty() {
                let start = reader.stream_position()?;
                let mut decoder = zstd::stream::read::Decoder::with_buffer(reader)?.single_frame();
                let mut raw = Vec::new();
                decoder
                    .by_ref()
                    .take(MAX_RECORD_BYTES + self.segment_bytes + 1)
                    .read_to_end(&mut raw)?;
                ensure!(
                    raw.len() as u64 <= MAX_RECORD_BYTES + self.segment_bytes,
                    "oversized message frame"
                );
                reader = decoder.finish();
                let end = reader.stream_position()?;
                ensure!(end > start, "message frame did not advance");
                let mut offset = 0;
                for line in raw.split_inclusive(|byte| *byte == b'\n') {
                    ensure!(
                        line.last() == Some(&b'\n'),
                        "incomplete sealed message record"
                    );
                    let record: Record = serde_json::from_slice(line)?;
                    check_record(&record, &mut chat, &mut previous, path)?;
                    insert_location(
                        &tx,
                        &record,
                        &Location {
                            path: path.into(),
                            compressed: true,
                            frame: start,
                            frame_len: end - start,
                            raw_len: raw.len() as u64,
                            offset,
                            length: line.len() as u64,
                        },
                    )?;
                    offset += line.len() as u64;
                }
            }
        } else {
            let file = OpenOptions::new().read(true).write(true).open(&full)?;
            let mut reader = BufReader::new(file);
            loop {
                let start = reader.stream_position()?;
                let mut raw = Vec::new();
                let read = reader
                    .by_ref()
                    .take(MAX_RECORD_BYTES + 1)
                    .read_until(b'\n', &mut raw)?;
                if read == 0 {
                    break;
                }
                ensure!(
                    raw.len() as u64 <= MAX_RECORD_BYTES,
                    "oversized message record"
                );
                if raw.last() != Some(&b'\n') {
                    reader.get_ref().set_len(start)?;
                    reader.get_ref().sync_all()?;
                    break;
                }
                let record: Record = serde_json::from_slice(&raw)?;
                check_record(&record, &mut chat, &mut previous, path)?;
                insert_location(
                    &tx,
                    &record,
                    &Location {
                        path: path.into(),
                        compressed: false,
                        frame: 0,
                        frame_len: 0,
                        raw_len: 0,
                        offset: start,
                        length: raw.len() as u64,
                    },
                )?;
            }
        }
        let chat = chat.unwrap_or_else(|| path.split('/').next().unwrap().into());
        register_file(&tx, path, &chat, &full)?;
        tx.commit()?;
        Ok(())
    }

    fn seal(&self, index: &mut Index, path: &str) -> Result<()> {
        let source = self.root.join(path);
        let target = self.root.join(format!("{path}.zst"));
        let temporary = self
            .root
            .join(format!("{path}.zst.tmp-{}", ulid::Ulid::new()));
        let result = (|| -> Result<()> {
            let mut reader = BufReader::new(File::open(&source)?);
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            let mut chunk = Vec::new();
            let mut expected = blake3::Hasher::new();
            loop {
                let mut line = Vec::new();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                ensure!(
                    line.last() == Some(&b'\n'),
                    "cannot seal incomplete message tail"
                );
                if !chunk.is_empty() && chunk.len() + line.len() > FRAME_BYTES {
                    encode_frame(&mut output, &chunk)?;
                    chunk.clear();
                }
                expected.update(&line);
                chunk.extend_from_slice(&line);
            }
            if !chunk.is_empty() {
                encode_frame(&mut output, &chunk)?;
            }
            output.sync_all()?;
            drop(output);
            let mut decoder = zstd::stream::read::Decoder::new(File::open(&temporary)?)?;
            let mut actual = blake3::Hasher::new();
            let mut buffer = [0; 64 * 1024];
            loop {
                let len = decoder.read(&mut buffer)?;
                if len == 0 {
                    break;
                }
                actual.update(&buffer[..len]);
            }
            ensure!(
                expected.finalize() == actual.finalize(),
                "compressed message segment verification failed"
            );
            fs::rename(&temporary, &target)?;
            sync_directory(target.parent().unwrap())?;
            // Rebuild locations against the complete new representation before
            // removing the old file. Readers share this lock and keep no paths.
            index
                .conn
                .execute("DELETE FROM records WHERE path=?1", [path])?;
            index
                .conn
                .execute("DELETE FROM files WHERE path=?1", [path])?;
            self.index_file(index, &format!("{path}.zst"))?;
            fs::remove_file(&source)?;
            sync_directory(source.parent().unwrap())?;
            Ok(())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn component(value: &str) -> Result<&str> {
    ensure!(
        !value.is_empty()
            && value.len() <= 240
            && !matches!(value, "." | "..")
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        "invalid Chat directory identity"
    );
    Ok(value)
}
fn stamp(path: &Path) -> Result<(u64, String)> {
    let meta = fs::metadata(path)?;
    Ok((
        meta.len(),
        meta.modified()?
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .to_string(),
    ))
}
fn register_file(conn: &Connection, path: &str, chat: &str, full: &Path) -> Result<()> {
    let (bytes, modified) = stamp(full)?;
    conn.execute("INSERT INTO files VALUES(?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET bytes=excluded.bytes,modified=excluded.modified",params![path,chat,bytes,modified])?;
    Ok(())
}
fn location(conn: &Connection, sequence: i64) -> Result<Option<Location>> {
    Ok(conn.query_row("SELECT path,compressed,frame,frame_len,raw_len,offset,length FROM records WHERE sequence=?1",[sequence],|row|Ok(Location {
        path:row.get(0)?,compressed:row.get(1)?,frame:row.get(2)?,frame_len:row.get(3)?,raw_len:row.get(4)?,offset:row.get(5)?,length:row.get(6)?,
    })).optional()?)
}
fn insert_location(conn: &Connection, record: &Record, location: &Location) -> Result<()> {
    conn.execute(
        "INSERT INTO records VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            record.sequence,
            record.message.chat_id,
            record.message.message_id,
            location.path,
            location.compressed,
            location.frame,
            location.frame_len,
            location.raw_len,
            location.offset,
            location.length,
        ],
    )?;
    Ok(())
}
fn check_record(
    record: &Record,
    chat: &mut Option<String>,
    previous: &mut i64,
    path: &str,
) -> Result<()> {
    ensure!(record.sequence > *previous, "message source order changed");
    ensure!(
        path.starts_with(&format!(
            "{}/messages/",
            component(&record.message.chat_id)?
        )),
        "message in wrong Chat directory"
    );
    if let Some(chat) = chat {
        ensure!(
            *chat == record.message.chat_id,
            "mixed Chat message segment"
        );
    } else {
        *chat = Some(record.message.chat_id.clone());
    }
    *previous = record.sequence;
    Ok(())
}
fn encode_frame(output: &mut File, raw: &[u8]) -> Result<()> {
    let mut encoder = zstd::stream::write::Encoder::new(output, 3)?;
    encoder.include_checksum(true)?;
    encoder.write_all(raw)?;
    encoder.finish()?;
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_client_types::chat::{Author, AuthorKind};
    fn record(sequence: i64) -> Record {
        Record {
            sequence,
            session_key: "local:chat".into(),
            role: "user".into(),
            kind: Some("message".into()),
            pages: vec![],
            message: Message {
                client_id: Some("origin/client".into()),
                message_id: format!("m{sequence}"),
                chat_id: "chat".into(),
                author: Author {
                    id: "user".into(),
                    kind: AuthorKind::User,
                    name: None,
                },
                text: format!("message {sequence}: {}", "长期保存的正文\n".repeat(20)),
                attachments: vec![],
                mentions: vec![],
                // Even records are replies with a quote; odd ones are old-style.
                reply_to: (sequence % 2 == 0).then(|| format!("m{}", sequence - 1)),
                quote: (sequence % 2 == 0).then(|| format!("引用 {sequence}")),
                quote_kind: (sequence % 4 == 0)
                    .then_some(zork_client_types::chat::QuoteKind::Summary),
                author_model: (sequence % 3 == 0).then(|| "claude-sonnet-5".into()),
                interaction: None,
                created_at: "2026-09-14T00:00:00Z".into(),
            },
        }
    }
    #[test]
    fn independent_frames_and_damaged_cache_rebuild_without_changing_epoch() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let log = MessageLog::open(root.path(), cache.path()).unwrap();
        let epoch = log.epoch("chat").unwrap();
        assert!(
            MessageLog::open(root.path(), cache.path()).is_err(),
            "concurrent source writer admitted"
        );
        let records = (1..=80)
            .map(|sequence| {
                let mut row = record(sequence);
                row.message.text = "multiple independent frames\n".repeat(500);
                row
            })
            .collect::<Vec<_>>();
        log.append(&records).unwrap();
        let mut index = log.state.lock().unwrap();
        let path = location(&index.conn, 1).unwrap().unwrap().path;
        log.seal(&mut index, &path).unwrap();
        assert!(
            index
                .conn
                .query_row("SELECT COUNT(DISTINCT frame) FROM records", [], |r| r
                    .get::<_, u64>(0))
                .unwrap()
                > 1
        );
        drop(index);
        for sequence in [1, 19, 20, 39, 80] {
            assert_eq!(
                log.get(sequence).unwrap().message,
                records[sequence as usize - 1].message
            );
        }
        assert!(log.state.lock().unwrap().bytes <= DECODE_CACHE_BYTES);
        drop(log);
        fs::write(
            cache.path().join("chat-message-locations.sqlite"),
            b"damaged location cache",
        )
        .unwrap();
        let log = MessageLog::open(root.path(), cache.path()).unwrap();
        assert_eq!(log.epoch("chat").unwrap(), epoch);
        assert_eq!(log.positions(0, 100).unwrap().len(), 80);
        assert_eq!(log.get(39).unwrap().message, records[38].message);
    }

    #[test]
    #[ignore = "standalone message storage performance measurement"]
    fn one_hundred_thousand_messages_indexed_page_cost() {
        use std::time::Instant;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let log = MessageLog::open(root.path(), cache.path()).unwrap();
        let started = Instant::now();
        for start in (1..=100_000).step_by(128) {
            let records = (start..(start + 128).min(100_001))
                .map(|sequence| {
                    let mut row = record(sequence);
                    row.message.text.push_str(&format!(
                        "\n```rust\nlet value = {sequence};\n```\n{}",
                        blake3::hash(sequence.to_string().as_bytes())
                    ));
                    row
                })
                .collect::<Vec<_>>();
            log.append(&records).unwrap();
        }
        let append_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut pages = Vec::new();
        let mut random = 17u64;
        for _ in 0..120 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let start = 1 + (random % 99_900) as i64;
            let now = Instant::now();
            for sequence in start..start + 100 {
                assert_eq!(log.get(sequence).unwrap().sequence, sequence);
            }
            pages.push(now.elapsed().as_secs_f64() * 1000.0);
        }
        pages.sort_by(f64::total_cmp);
        let index = log.state.lock().unwrap();
        let source_bytes: u64 = index
            .conn
            .query_row("SELECT SUM(bytes) FROM files", [], |r| r.get(0))
            .unwrap();
        let segments: u64 = index
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        let cache_bytes: u64 = fs::read_dir(cache.path())
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        println!(
            "{}",
            serde_json::json!({"records":100000,"page_messages":100,"pages":120,"append_ms":append_ms,
            "page_p50_ms":pages[60],"page_p95_ms":pages[114],"page_p99_ms":pages[118],"source_bytes":source_bytes,
            "index_bytes":cache_bytes,"segments":segments,"decoded_cache_bytes":index.bytes,"fixture":"mixed Chinese, code, IDs; warm OS cache; dev profile"})
        );
    }
    #[test]
    fn sealed_frames_rebuild_from_source_and_preserve_source_positions() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let source = root.path().join("chats");
        let mut log = MessageLog::open(&source, &cache).unwrap();
        log.segment_bytes = 4096;
        for i in 1..=40 {
            log.append(&[record(i)]).unwrap();
        }
        assert!(fs::read_dir(source.join("chat/messages"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|value| value == "zst")));
        for i in [1, 19, 40, 8] {
            assert_eq!(log.get(i).unwrap().message.text, record(i).message.text);
        }
        // Compression keeps reply quotes and leaves old records without them.
        for i in [2, 4, 7, 40] {
            assert_eq!(log.get(i).unwrap().message, record(i).message);
        }
        assert_eq!(
            log.get(4).unwrap().message.quote_kind,
            Some(zork_client_types::chat::QuoteKind::Summary)
        );
        assert_eq!(log.get(7).unwrap().message.quote, None);
        drop(log);
        fs::remove_dir_all(&cache).unwrap();
        let log = MessageLog::open(&source, &cache).unwrap();
        assert_eq!(log.positions(0, 100).unwrap(), (1..=40).collect::<Vec<_>>());
        for i in [40, 1, 9, 20] {
            assert_eq!(log.get(i).unwrap().message.text, record(i).message.text);
            assert_eq!(log.get(i).unwrap().message, record(i).message);
        }
    }
    #[test]
    fn torn_active_tail_is_removed_but_committed_record_corruption_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("chats");
        let cache = root.path().join("cache");
        let log = MessageLog::open(&source, &cache).unwrap();
        log.append(&[record(1)]).unwrap();
        drop(log);
        let path = source.join("chat/messages/00000000000000000001.jsonl");
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"sequence\":2")
            .unwrap();
        let log = MessageLog::open(&source, &cache).unwrap();
        log.append(&[record(2)]).unwrap();
        drop(log);
        let log = MessageLog::open(&source, &cache).unwrap();
        assert_eq!(log.positions(0, 10).unwrap(), [1, 2]);
        drop(log);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"bad record\n")
            .unwrap();
        assert!(MessageLog::open(&source, &cache).is_err());
    }
    #[test]
    fn recovery_uses_source_position_and_separate_sends_remain_separate() {
        let root = tempfile::tempdir().unwrap();
        let log = MessageLog::open(&root.path().join("chats"), &root.path().join("cache")).unwrap();
        let first = record(1);
        log.append(&[first.clone()]).unwrap();
        log.append(&[first.clone()]).unwrap();
        let mut other = record(2);
        other.message.text = first.message.text.clone();
        log.append(&[other]).unwrap();
        assert_eq!(log.positions(0, 10).unwrap(), [1, 2]);
        let mut changed = first;
        changed.message.text = "changed".into();
        assert!(log.append(&[changed]).is_err());
        let mut unsafe_path = record(3);
        unsafe_path.message.chat_id = "../escape".into();
        assert!(log.append(&[unsafe_path]).is_err());
    }
}
