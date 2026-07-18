//! Portable local IPC: Unix domain sockets on unix, named pipes on Windows.
//!
//! Everything that used to talk `tokio::net::UnixListener`/`UnixStream` directly (the daemon's
//! socket server, the shared [`crate::client::DaemonClient`], tests) goes through this module
//! instead, so the JSON-RPC framing in [`crate::protocol`] runs unchanged over whichever
//! transport the platform provides. Only the byte pipe differs per OS; the wire protocol is
//! identical (and frozen — the iOS companion mirrors it).
//!
//! Endpoints are still configured as paths (`config::socket_path`). On unix that path is the
//! socket file; on Windows it is interpreted as a named-pipe name (see
//! [`pipe_name_from_path`]).

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A resolved local IPC endpoint: a socket path on unix, a pipe name on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    #[cfg(unix)]
    Unix(std::path::PathBuf),
    #[cfg(windows)]
    Pipe(String),
}

impl Endpoint {
    /// Interpret a configured "socket path" for this platform.
    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            Endpoint::Unix(path.to_path_buf())
        }
        #[cfg(windows)]
        {
            Endpoint::Pipe(pipe_name_from_path(path))
        }
    }
}

/// Map a configured "socket path" to a Windows named-pipe name.
///
/// A value that already names a pipe (`\\.\pipe\...`) is used verbatim — the default
/// `config::default_socket_path()` on Windows produces exactly that. Anything else (say a
/// unix-style `socket_path` override carried over in a shared config) is flattened into a pipe
/// name: path separators and other non-name characters become `-`, and the `\\.\pipe\` prefix
/// is prepended. Pure string logic so it is unit-testable on every OS.
pub fn pipe_name_from_path(path: &Path) -> String {
    const PIPE_PREFIX: &str = r"\\.\pipe\";
    let s = path.to_string_lossy();
    if s.starts_with(PIPE_PREFIX) {
        return s.into_owned();
    }
    let flat: String = s
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | ' ' => '-',
            other => other,
        })
        .collect();
    format!("{PIPE_PREFIX}{}", flat.trim_matches('-'))
}

enum ListenerInner {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg(windows)]
    Pipe {
        name: String,
        /// The pre-created pipe instance the next client will hit. Windows named pipes have no
        /// single listening object: each accepted connection consumes one server instance, so
        /// `accept` creates the following instance *before* handing the connected one out —
        /// that way there is never a window with no instance for a client to reach.
        next: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    },
}

/// A bound local IPC listener. Obtain with [`listen`], then call [`IpcListener::accept`].
pub struct IpcListener {
    inner: ListenerInner,
}

/// Bind a local IPC listener at `endpoint`.
///
/// Unix: creates the parent directory and clears a stale socket file from a previous run
/// before binding. Windows: creates the first pipe instance (exclusively — a second listener
/// on the same name fails, mirroring the unix bind conflict) and rejects remote clients.
pub async fn listen(endpoint: &Endpoint) -> io::Result<IpcListener> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
            Ok(IpcListener {
                inner: ListenerInner::Unix(tokio::net::UnixListener::bind(path)?),
            })
        }
        #[cfg(windows)]
        Endpoint::Pipe(name) => {
            use tokio::net::windows::named_pipe::ServerOptions;
            let first = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(name)?;
            Ok(IpcListener {
                inner: ListenerInner::Pipe {
                    name: name.clone(),
                    next: Some(first),
                },
            })
        }
    }
}

impl IpcListener {
    /// Wait for and return the next client connection.
    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        match &mut self.inner {
            #[cfg(unix)]
            ListenerInner::Unix(listener) => {
                let (stream, _addr) = listener.accept().await?;
                Ok(IpcStream::Unix(stream))
            }
            #[cfg(windows)]
            ListenerInner::Pipe { name, next } => {
                use tokio::net::windows::named_pipe::ServerOptions;
                let server = match next.take() {
                    Some(server) => server,
                    // The previous accept failed to pre-create an instance (e.g. a transient
                    // resource error); retry here rather than being wedged forever.
                    None => ServerOptions::new()
                        .reject_remote_clients(true)
                        .create(&*name)?,
                };
                server.connect().await?;
                *next = ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&*name)
                    .ok();
                Ok(IpcStream::PipeServer(server))
            }
        }
    }
}

/// Connect to a local IPC endpoint.
///
/// Windows: `ERROR_PIPE_BUSY` (every instance momentarily taken — the accept loop pre-creates
/// the next instance, so this is a tiny race window) is retried briefly with backoff; other
/// errors (notably "not found" while the daemon is still starting) surface immediately so
/// callers' existing connect-retry loops behave exactly as they do on unix.
pub async fn connect(endpoint: &Endpoint) -> io::Result<IpcStream> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => Ok(IpcStream::Unix(
            tokio::net::UnixStream::connect(path).await?,
        )),
        #[cfg(windows)]
        Endpoint::Pipe(name) => {
            use tokio::net::windows::named_pipe::ClientOptions;
            const ERROR_PIPE_BUSY: i32 = 231;
            let mut delay = std::time::Duration::from_millis(10);
            let mut waited = std::time::Duration::ZERO;
            const BUSY_CEILING: std::time::Duration = std::time::Duration::from_secs(2);
            loop {
                match ClientOptions::new().open(name) {
                    Ok(client) => return Ok(IpcStream::PipeClient(client)),
                    Err(e)
                        if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && waited < BUSY_CEILING =>
                    {
                        tokio::time::sleep(delay).await;
                        waited += delay;
                        delay = (delay * 2).min(std::time::Duration::from_millis(100));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

/// A connected local IPC stream: `AsyncRead + AsyncWrite + Unpin + Send`, whatever the
/// platform transport underneath. The `Duplex` variant is an in-memory pair for tests
/// ([`IpcStream::pair`]), replacing the old `UnixStream::pair()`.
pub enum IpcStream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    PipeServer(tokio::net::windows::named_pipe::NamedPipeServer),
    #[cfg(windows)]
    PipeClient(tokio::net::windows::named_pipe::NamedPipeClient),
    Duplex(tokio::io::DuplexStream),
}

impl IpcStream {
    /// An in-memory connected pair (for tests), like `UnixStream::pair()` but portable.
    pub fn pair() -> (IpcStream, IpcStream) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (IpcStream::Duplex(a), IpcStream::Duplex(b))
    }
}

macro_rules! with_stream {
    ($self:ident, $s:ident => $e:expr) => {
        match Pin::get_mut($self) {
            #[cfg(unix)]
            IpcStream::Unix($s) => $e,
            #[cfg(windows)]
            IpcStream::PipeServer($s) => $e,
            #[cfg(windows)]
            IpcStream::PipeClient($s) => $e,
            IpcStream::Duplex($s) => $e,
        }
    };
}

impl AsyncRead for IpcStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        with_stream!(self, s => Pin::new(s).poll_read(cx, buf))
    }
}

impl AsyncWrite for IpcStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        with_stream!(self, s => Pin::new(s).poll_write(cx, buf))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        with_stream!(self, s => Pin::new(s).poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        with_stream!(self, s => Pin::new(s).poll_shutdown(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_frame, write_frame};

    /// A unique per-test endpoint on whichever transport this platform uses.
    fn test_endpoint(tag: &str) -> Endpoint {
        #[cfg(unix)]
        {
            Endpoint::Unix(
                std::env::temp_dir().join(format!("repomon-tr-{tag}-{}.sock", std::process::id())),
            )
        }
        #[cfg(windows)]
        {
            Endpoint::Pipe(format!(r"\\.\pipe\repomon-tr-{tag}-{}", std::process::id()))
        }
    }

    /// The core contract: a length-prefixed protocol frame round-trips over the real platform
    /// transport (UDS here on unix, a named pipe on Windows CI).
    #[tokio::test]
    async fn round_trips_a_frame_over_the_platform_transport() {
        let ep = test_endpoint("rt");
        let mut listener = listen(&ep).await.unwrap();
        let server = tokio::spawn(async move {
            let mut s = listener.accept().await.unwrap();
            let frame = read_frame(&mut s).await.unwrap().expect("a frame");
            write_frame(&mut s, &frame).await.unwrap(); // echo it back
        });

        let mut client = connect(&ep).await.unwrap();
        let payload = br#"{"jsonrpc":"2.0","method":"ping","id":1}"#;
        write_frame(&mut client, payload).await.unwrap();
        let echoed = read_frame(&mut client).await.unwrap().expect("echo");
        assert_eq!(echoed, payload);
        server.await.unwrap();
    }

    /// The listener must keep accepting: two clients in a row (this exercises the Windows
    /// "pre-create the next pipe instance" path; on unix it is a plain double accept).
    #[tokio::test]
    async fn accepts_sequential_clients() {
        let ep = test_endpoint("seq");
        let mut listener = listen(&ep).await.unwrap();
        let server = tokio::spawn(async move {
            for i in 0..2u8 {
                let mut s = listener.accept().await.unwrap();
                write_frame(&mut s, &[b'0' + i]).await.unwrap();
            }
        });

        for i in 0..2u8 {
            let mut c = connect(&ep).await.unwrap();
            let got = read_frame(&mut c).await.unwrap().expect("frame");
            assert_eq!(got, vec![b'0' + i]);
        }
        server.await.unwrap();
    }

    /// The in-memory pair used by unit tests behaves like a connected socket.
    #[tokio::test]
    async fn round_trips_over_a_duplex_pair() {
        let (mut a, mut b) = IpcStream::pair();
        write_frame(&mut a, b"hello").await.unwrap();
        let got = read_frame(&mut b).await.unwrap().expect("frame");
        assert_eq!(got, b"hello");
    }

    /// Pipe-name mapping is pure string logic, verified on every OS.
    #[test]
    fn pipe_name_mapping() {
        // A real pipe name passes through verbatim.
        assert_eq!(
            pipe_name_from_path(Path::new(r"\\.\pipe\repomon-ali")),
            r"\\.\pipe\repomon-ali"
        );
        // A unix-style path is flattened into a name under \\.\pipe\.
        assert_eq!(
            pipe_name_from_path(Path::new("/tmp/repomon-ali.sock")),
            r"\\.\pipe\tmp-repomon-ali.sock"
        );
        // Windows filesystem paths lose the drive colon and separators too.
        assert_eq!(
            pipe_name_from_path(Path::new(r"C:\Temp\repomon.sock")),
            r"\\.\pipe\C--Temp-repomon.sock"
        );
    }
}
