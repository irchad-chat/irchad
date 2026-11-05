//! What chadd fetched for previews and `/api/media`, kept on disk so a link
//! posted to a busy channel is fetched once, not once per reader. Concurrent
//! requests for the same key share one fetch, even when nothing is on disk.
//!
//! Keys are URLs from anyone on the network (D14): the total is capped (LRU),
//! so is each item, and a file stores its own key so a hash collision is a
//! miss, never someone else's bytes.

use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use axum::http::StatusCode;
use tokio::sync::watch;

use crate::config;

/// Pages and images change; a day old is stale.
const TTL: Duration = Duration::from_secs(24 * 3600);
/// What a file costs at least, so a flood of tiny items can't grow the index
/// past what the byte cap suggests.
const BLOCK: u64 = 4096;

pub type Fetched = Result<Bytes, StatusCode>;

#[derive(Default)]
pub struct Cache {
    disk: Option<Arc<Disk>>,
    inflight: Arc<Mutex<HashMap<String, watch::Receiver<Option<Fetched>>>>>,
}

struct Disk {
    dir: PathBuf,
    max_bytes: u64,
    max_item: u64,
    tmp: AtomicU64,
    index: Mutex<Index>,
}

#[derive(Default)]
struct Index {
    /// File name → (charged size, last used).
    files: HashMap<String, (u64, SystemTime)>,
    total: u64,
}

impl Cache {
    /// Without `config`, fetches are only shared while in flight.
    pub fn new(config: Option<&config::Cache>) -> anyhow::Result<Self> {
        let Some(c) = config else {
            return Ok(Self::default());
        };
        std::fs::create_dir_all(&c.dir)?;
        let mut index = Index::default();
        for e in std::fs::read_dir(&c.dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let meta = e.metadata()?;
            if name.ends_with(".tmp") || !meta.is_file() {
                let _ = std::fs::remove_file(e.path());
                continue;
            }
            let size = charge(meta.len());
            index.total += size;
            index.files.insert(name, (size, meta.modified()?));
        }
        let disk = Disk {
            dir: c.dir.clone(),
            max_bytes: c.max_bytes,
            max_item: c.max_item,
            tmp: AtomicU64::new(0),
            index: Mutex::new(index),
        };
        disk.evict(&mut disk.lock());
        Ok(Self {
            disk: Some(Arc::new(disk)),
            inflight: Arc::default(),
        })
    }

    /// `key`'s bytes from disk, else from `fetch`, run at most once at a time
    /// per key however many ask. Errors are shared with whoever was waiting,
    /// not kept.
    pub async fn get<F>(&self, key: String, fetch: F) -> Fetched
    where
        F: Future<Output = Fetched> + Send + 'static,
    {
        if let Some(d) = &self.disk
            && let Some(hit) = d.read(&key).await
        {
            return Ok(hit);
        }
        let mut rx = {
            let mut inflight = crate::lock(&self.inflight);
            match inflight.get(&key) {
                Some(rx) => rx.clone(),
                None => {
                    let (tx, rx) = watch::channel(None);
                    inflight.insert(key.clone(), rx.clone());
                    // Spawned, so it finishes (and leaves the map) even if
                    // every requester hangs up. reqwest's timeout bounds it.
                    let (disk, inflight) = (self.disk.clone(), self.inflight.clone());
                    tokio::spawn(async move {
                        let res = fetch.await;
                        if let (Some(d), Ok(body)) = (disk, &res)
                            && body.len() as u64 <= d.max_item
                        {
                            let (key, body) = (key.clone(), body.clone());
                            let _ = tokio::task::spawn_blocking(move || d.write(&key, &body)).await;
                        }
                        let _ = tx.send(Some(res));
                        crate::lock(&inflight).remove(&key);
                    });
                    rx
                }
            }
        };
        match rx.wait_for(Option::is_some).await {
            Ok(res) => res.clone().unwrap_or(Err(StatusCode::BAD_GATEWAY)),
            // The fetch panicked.
            Err(_) => Err(StatusCode::BAD_GATEWAY),
        }
    }
}

fn charge(len: u64) -> u64 {
    len.div_ceil(BLOCK).max(1) * BLOCK
}

fn name(key: &str) -> String {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Disk {
    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        crate::lock(&self.index)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// File layout: key length (u32 LE), key, body.
    async fn read(&self, key: &str) -> Option<Bytes> {
        let name = name(key);
        let path = self.path(&name);
        let fresh = tokio::fs::metadata(&path)
            .await
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .is_ok_and(|age| age < TTL);
        if !fresh {
            return None;
        }
        let data = tokio::fs::read(&path).await.ok()?;
        let start = data.len() - parse(&data, key)?.len();
        if let Some((_, used)) = self.lock().files.get_mut(&name) {
            *used = SystemTime::now();
        }
        // The body in place, without copying up to `max_item` bytes again.
        Some(Bytes::from(data).slice(start..))
    }

    fn write(&self, key: &str, body: &[u8]) {
        if let Err(e) = self.try_write(key, body) {
            tracing::warn!("cache write: {e}");
        }
    }

    fn try_write(&self, key: &str, body: &[u8]) -> std::io::Result<()> {
        let name = name(key);
        let len = u32::try_from(key.len()).map_err(|_| ErrorKind::InvalidInput)?;
        let tmp = self.path(&format!(
            "{name}.{}.tmp",
            self.tmp.fetch_add(1, Ordering::Relaxed)
        ));
        let mut data = Vec::with_capacity(4 + key.len() + body.len());
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(key.as_bytes());
        data.extend_from_slice(body);
        std::fs::write(&tmp, &data)?;
        // Renames and evictions both happen under the lock, so they can't race.
        let mut index = self.lock();
        if let Err(e) = std::fs::rename(&tmp, self.path(&name)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        let size = charge(data.len() as u64);
        if let Some((old, _)) = index.files.insert(name, (size, SystemTime::now())) {
            index.total -= old;
        }
        index.total += size;
        self.evict(&mut index);
        Ok(())
    }

    /// Least recently used first, down to 90% so this runs rarely.
    fn evict(&self, index: &mut Index) {
        if index.total <= self.max_bytes {
            return;
        }
        let mut by_age: Vec<_> = index
            .files
            .iter()
            .map(|(n, &(s, t))| (t, s, n.clone()))
            .collect();
        by_age.sort_unstable();
        let target = self.max_bytes / 10 * 9;
        for (_, size, name) in by_age {
            if index.total <= target {
                break;
            }
            match std::fs::remove_file(self.path(&name)) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("cache evict: {e}"),
            }
            index.files.remove(&name);
            index.total -= size;
        }
    }
}

/// The body, if `data` is a well-formed file for `key`.
fn parse<'a>(data: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let len = u32::from_le_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let stored = data.get(4..4usize.checked_add(len)?)?;
    (stored == key.as_bytes()).then(|| &data[4 + len..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    fn cache(dir: &Path, max_bytes: u64, max_item: u64) -> Cache {
        Cache::new(Some(&config::Cache {
            dir: dir.into(),
            max_bytes,
            max_item,
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn fetches_once_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let c = Arc::new(cache(dir.path(), 1 << 20, 1 << 10));
        let calls = Arc::new(AtomicUsize::new(0));
        let get = |c: Arc<Cache>, calls: Arc<AtomicUsize>| async move {
            c.get("k".into(), async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(Bytes::from_static(b"body"))
            })
            .await
        };
        let all: Vec<_> = (0..10)
            .map(|_| tokio::spawn(get(c.clone(), calls.clone())))
            .collect();
        for h in all {
            assert_eq!(h.await.unwrap().unwrap(), "body");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // From disk now, including after a restart.
        let c = Arc::new(cache(dir.path(), 1 << 20, 1 << 10));
        assert_eq!(get(c, calls.clone()).await.unwrap(), "body");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn caps_items_and_total() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(dir.path(), 4 * BLOCK, 2 * BLOCK);
        let big = Bytes::from(vec![0; 3 * BLOCK as usize]);
        assert!(c.get("big".into(), async move { Ok(big) }).await.is_ok());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        for i in 0..10 {
            c.get(format!("k{i}"), async { Ok(Bytes::from_static(b"x")) })
                .await
                .unwrap();
        }
        let files = std::fs::read_dir(dir.path()).unwrap().count() as u64;
        assert!(files <= 4, "{files}");
        let d = c.disk.as_ref().unwrap();
        assert!(d.lock().total <= 4 * BLOCK);
        assert!(d.read("k9").await.is_some());
        assert!(d.read("k0").await.is_none());
        // Errors aren't kept.
        let e = c
            .get("e".into(), async { Err(StatusCode::BAD_GATEWAY) })
            .await;
        assert_eq!(e, Err(StatusCode::BAD_GATEWAY));
        assert!(d.read("e").await.is_none());
    }

    #[test]
    fn junk_files_are_misses() {
        for junk in [&b""[..], b"\xff\xff\xff\xff", b"\x01\0\0\0", b"\x01\0\0\0j"] {
            assert_eq!(parse(junk, "k"), None);
        }
        assert_eq!(parse(b"\x01\0\0\0kbody", "k"), Some(&b"body"[..]));
    }
}
