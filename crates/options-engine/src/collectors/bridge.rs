//! Layer 2 (part) — app bridges over a unix socket.
//!
//! Apps that *want* to tell OPTIONS what they're doing connect to
//! `$XDG_RUNTIME_DIR/options/bridge.sock` and send line-delimited, versioned
//! JSON. These are the highest-signal, lowest-guesswork inputs — the shell
//! knows its last command and exit code, the editor knows the file/language and
//! diagnostics, the browser knows the URL and whether it's on docs.
//!
//! This is the **engine side**: the socket, the protocol, and the merge. The
//! client hooks (zsh/fish, an editor plugin/LSP shim, a browser native-messaging
//! host) are installed separately and just write these lines.
//!
//! ## Protocol (v1)
//! One JSON object per line, tagged by `kind`; unknown fields are ignored so it
//! can grow compatibly:
//! ```json
//! {"v":1,"kind":"shell","last_cmd":"cargo build","exit_code":0}
//! {"v":1,"kind":"editor","file":"/home/max/p/src/main.rs","language":"rust","diagnostics":2}
//! {"v":1,"kind":"browser","url":"https://developer.mozilla.org/…","reading_docs":true}
//! ```
//! Each `kind` owns its slice of [`AppInternalContext`] and replaces it wholesale
//! on every message; the merged whole is published on each update.

use std::path::PathBuf;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{AppInternalContext, ContextState, Layer};

use std::time::Duration;

const RECONNECT: Duration = Duration::from_secs(2);

/// Layer-2 bridge server. Optionally overrides the socket path (tests).
#[derive(Default)]
pub struct BridgeCollector {
    socket: Option<PathBuf>,
}

impl BridgeCollector {
    pub fn new() -> Self {
        Self { socket: None }
    }
}

impl Collector for BridgeCollector {
    fn name(&self) -> &'static str {
        "bridge"
    }
    fn layer(&self) -> Layer {
        Layer::AppBridge
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            let path = self.socket.unwrap_or_else(default_socket_path);
            loop {
                let listener = match bind(&path) {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::debug!("bridge: bind {} failed: {e}", path.display());
                        tokio::time::sleep(RECONNECT).await;
                        continue;
                    }
                };
                let _ = tx.send(Update::Health(Layer::AppBridge, true)).await;
                serve(listener, tx.clone()).await;
                let _ = tx.send(Update::Health(Layer::AppBridge, false)).await;
                tokio::time::sleep(RECONNECT).await;
            }
        })
    }
}

/// `$XDG_RUNTIME_DIR/options/bridge.sock` (falls back to `/tmp`).
fn default_socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("options").join("bridge.sock")
}

/// Create the parent dir, clear any stale socket, and bind.
fn bind(path: &std::path::Path) -> std::io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A leftover socket from a previous run would make bind fail with EADDRINUSE.
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path)
}

/// Accept connections and merge their messages until the listener dies.
async fn serve(listener: UnixListener, tx: mpsc::Sender<Update>) {
    let (msg_tx, mut msg_rx) = mpsc::channel::<BridgeMsg>(64);

    // Accept loop: one reader task per client.
    let accept = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(handle_conn(stream, msg_tx.clone()));
                }
                Err(e) => {
                    tracing::debug!("bridge: accept failed: {e}");
                    break;
                }
            }
        }
    });

    // Merge loop: fold each message into the running app-internal context.
    let mut state = AppInternalContext::default();
    while let Some(msg) = msg_rx.recv().await {
        apply(&mut state, msg);
        if tx
            .send(Update::Delta(
                Layer::AppBridge,
                ContextDelta::AppInternal(state.clone()),
            ))
            .await
            .is_err()
        {
            break; // aggregator gone
        }
    }
    accept.abort();
}

/// Read line-delimited JSON from one client, forwarding parsed messages.
async fn handle_conn(stream: UnixStream, msg_tx: mpsc::Sender<BridgeMsg>) {
    let mut lines = BufReader::new(stream).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if !line.trim().is_empty() => {
                match serde_json::from_str::<BridgeMsg>(&line) {
                    Ok(msg) => {
                        if msg_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::debug!("bridge: ignoring bad message: {e}"),
                }
            }
            Ok(Some(_)) => {}  // blank line
            Ok(None) => break, // client closed
            Err(e) => {
                tracing::debug!("bridge: read error: {e}");
                break;
            }
        }
    }
}

/// A bridge message. `kind` selects the source; unknown fields are ignored so
/// the protocol can evolve. `v` (version) is accepted and currently ignored.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum BridgeMsg {
    Shell {
        #[serde(default)]
        last_cmd: Option<String>,
        #[serde(default)]
        exit_code: Option<i32>,
    },
    Editor {
        #[serde(default)]
        file: Option<PathBuf>,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        diagnostics: Option<u32>,
    },
    Browser {
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        reading_docs: Option<bool>,
    },
}

/// Fold one message into the merged context. Each `kind` replaces its own
/// fields wholesale (clients send their full relevant subset each time).
fn apply(state: &mut AppInternalContext, msg: BridgeMsg) {
    match msg {
        BridgeMsg::Shell {
            last_cmd,
            exit_code,
        } => {
            state.shell_last_cmd = last_cmd;
            state.shell_exit_code = exit_code;
        }
        BridgeMsg::Editor {
            file,
            language,
            diagnostics,
        } => {
            state.editor_file = file;
            state.editor_language = language;
            state.editor_diagnostics_count = diagnostics.unwrap_or(0);
        }
        BridgeMsg::Browser { url, reading_docs } => {
            state.browser_url = url;
            state.is_reading_docs = reading_docs.unwrap_or(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_kind_ignoring_version_and_extras() {
        let shell: BridgeMsg = serde_json::from_str(
            r#"{"v":1,"kind":"shell","last_cmd":"ls","exit_code":0,"cwd":"/x"}"#,
        )
        .unwrap();
        assert_eq!(
            shell,
            BridgeMsg::Shell {
                last_cmd: Some("ls".into()),
                exit_code: Some(0)
            }
        );
        let editor: BridgeMsg = serde_json::from_str(
            r#"{"kind":"editor","file":"/a.rs","language":"rust","diagnostics":3}"#,
        )
        .unwrap();
        assert!(matches!(
            editor,
            BridgeMsg::Editor {
                diagnostics: Some(3),
                ..
            }
        ));
    }

    #[test]
    fn each_kind_replaces_only_its_own_fields() {
        let mut s = AppInternalContext::default();
        apply(
            &mut s,
            BridgeMsg::Editor {
                file: Some("/a.rs".into()),
                language: Some("rust".into()),
                diagnostics: Some(2),
            },
        );
        apply(
            &mut s,
            BridgeMsg::Shell {
                last_cmd: Some("cargo build".into()),
                exit_code: Some(1),
            },
        );
        // Editor fields survive the shell update.
        assert_eq!(s.editor_language.as_deref(), Some("rust"));
        assert_eq!(s.editor_diagnostics_count, 2);
        assert_eq!(s.shell_exit_code, Some(1));
    }

    #[tokio::test]
    async fn serves_a_client_and_emits_merged_context() {
        let dir = std::env::temp_dir().join(format!("opt-bridge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("bridge.sock");
        let listener = bind(&sock).unwrap();

        let (tx, mut rx) = mpsc::channel::<Update>(8);
        tokio::spawn(serve(listener, tx));

        // A client connects and sends an editor message.
        use tokio::io::AsyncWriteExt;
        let mut client = UnixStream::connect(&sock).await.unwrap();
        client
            .write_all(b"{\"kind\":\"editor\",\"file\":\"/x.rs\",\"language\":\"rust\",\"diagnostics\":4}\n")
            .await
            .unwrap();

        let update = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("no update in time")
            .expect("channel closed");
        match update {
            Update::Delta(Layer::AppBridge, ContextDelta::AppInternal(ctx)) => {
                assert_eq!(
                    ctx.editor_file.as_deref(),
                    Some(std::path::Path::new("/x.rs"))
                );
                assert_eq!(ctx.editor_language.as_deref(), Some("rust"));
                assert_eq!(ctx.editor_diagnostics_count, 4);
            }
            other => panic!("unexpected update: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
