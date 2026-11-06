//! Rich Presence on a Unix socket, `discord-ipc-N` in `$XDG_RUNTIME_DIR`.

use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::UnixListener;
use tokio::task::JoinHandle;

use super::rpc::{Clients, SLOTS, Shared};

/// Listening; dropping it hangs up on every game and removes the socket.
pub struct Listener {
    task: JoinHandle<()>,
    path: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Where Discord's SDKs look first. Flatpak and Snap Discord use subdirectories
/// we don't serve.
fn dir() -> PathBuf {
    ["XDG_RUNTIME_DIR", "TMPDIR"]
        .iter()
        .find_map(std::env::var_os)
        .map_or_else(|| "/tmp".into(), PathBuf::from)
}

/// Takes the highest free slot: SDKs try from 0 up, so a Discord that starts
/// after us still gets the lower one and the games.
pub fn listen(shared: Arc<Shared>) -> Option<Listener> {
    let dir = dir();
    let (listener, path) = (0..SLOTS).rev().find_map(|i| {
        let path = dir.join(format!("discord-ipc-{i}"));
        match std::fs::symlink_metadata(&path) {
            // Someone answers (Discord, or another IrChad): theirs.
            Ok(m) if m.file_type().is_socket() => {
                if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                    return None;
                }
                // Nobody does: left behind by a crash.
                let _ = std::fs::remove_file(&path);
            }
            Ok(_) => return None,
            Err(_) => {}
        }
        let l = std::os::unix::net::UnixListener::bind(&path).ok()?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        l.set_nonblocking(true).ok()?;
        Some((UnixListener::from_std(l).ok()?, path))
    })?;
    // In a shared /tmp, someone could connect before the chmod: check who it is.
    let uid = std::fs::symlink_metadata(&path).ok()?.uid();
    tracing::info!("rich presence: listening on {}", path.display());
    let task = tokio::spawn(async move {
        let mut clients = Clients::new(shared);
        loop {
            match listener.accept().await {
                Ok((s, _)) if s.peer_cred().is_ok_and(|c| c.uid() == uid) => clients.add(s),
                Ok(_) => tracing::warn!("rich presence: another user connected; hung up"),
                Err(e) => {
                    tracing::warn!("rich presence: {e}");
                    return;
                }
            }
        }
    });
    Some(Listener { task, path })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn real_socket_answers_and_goes_away() {
        let (tx, mut rx) = tokio::sync::watch::channel(None);
        let shared = super::super::rpc::tests::shared_with(tx);
        let l = super::listen(shared).expect("a free slot");
        let path = l.path.clone();
        assert!(path.exists());

        let mut s = tokio::net::UnixStream::connect(&path).await.unwrap();
        let hello = br#"{"v":1,"client_id":"42"}"#;
        let mut frame = 0u32.to_le_bytes().to_vec();
        frame.extend_from_slice(&(hello.len() as u32).to_le_bytes());
        frame.extend_from_slice(hello);
        s.write_all(&frame).await.unwrap();
        let mut head = [0u8; 8];
        s.read_exact(&mut head).await.unwrap();
        assert_eq!(head[..4], 1u32.to_le_bytes());

        let act = br#"{"cmd":"SET_ACTIVITY","nonce":"1","args":{"activity":{"details":"x"}}}"#;
        let mut frame = 1u32.to_le_bytes().to_vec();
        frame.extend_from_slice(&(act.len() as u32).to_le_bytes());
        frame.extend_from_slice(act);
        s.write_all(&frame).await.unwrap();
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().as_ref().map(|a| a.app.as_str()), Some("42"));

        drop(l);
        assert!(!path.exists());
        let mut buf = [0u8; 1];
        // Our reply to SET_ACTIVITY may still be buffered; then EOF.
        while s.read(&mut buf).await.unwrap() != 0 {}
    }
}
