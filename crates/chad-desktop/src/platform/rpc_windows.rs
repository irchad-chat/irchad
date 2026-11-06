//! Rich Presence on a named pipe, `\\.\pipe\discord-ipc-N`. The default pipe
//! ACL gives other users read access only, and remote clients are refused.

use std::sync::Arc;

use tokio::net::windows::named_pipe::ServerOptions;
use tokio::task::JoinHandle;

use super::rpc::{Clients, SLOTS, Shared};

/// Listening; dropping it hangs up on every game and closes the pipe.
pub struct Listener(JoinHandle<()>);

impl Drop for Listener {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Takes the highest free slot: SDKs try from 0 up, so a Discord that starts
/// after us still gets the lower one and the games. `first_pipe_instance`
/// fails while someone else holds a name.
pub fn listen(shared: Arc<Shared>) -> Option<Listener> {
    let (name, mut server) = (0..SLOTS).rev().find_map(|i| {
        let name = format!(r"\\.\pipe\discord-ipc-{i}");
        let s = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .ok()?;
        Some((name, s))
    })?;
    tracing::info!("rich presence: listening on {name}");
    Some(Listener(tokio::spawn(async move {
        let mut clients = Clients::new(shared);
        loop {
            let next = match server.connect().await {
                Ok(()) => ServerOptions::new().create(&name),
                Err(e) => Err(e),
            };
            match next {
                Ok(next) => clients.add(std::mem::replace(&mut server, next)),
                Err(e) => {
                    tracing::warn!("rich presence: {e}");
                    return;
                }
            }
        }
    })))
}
