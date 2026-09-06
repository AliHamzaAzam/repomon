//! Carries the same framed daemon protocol over Unix sockets and Windows named pipes.

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

/// Preserves explicit Windows pipe names and maps other configured paths into the pipe namespace by
/// replacing invalid name characters.
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
        /// The per-user security descriptor every instance is created with (see
        /// [`pipe_instance`]): local IPC must not be reachable by other users.
        security: repomon_host::dacl::PipeSecurity,
        /// Create the next server instance before handing off the connection so clients always have
        /// a listener to connect to.
        next: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    },
}

/// Create a pipe with a protected current-user-only DACL; the first instance also claims the name
/// exclusively.
#[cfg(windows)]
fn pipe_instance(
    name: &str,
    security: &repomon_host::dacl::PipeSecurity,
    first: bool,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut attrs = security.attributes();
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                &mut attrs as *mut _ as *mut std::ffi::c_void,
            )
    }
}

/// A bound local IPC listener. Obtain with [`listen`], then call [`IpcListener::accept`].
pub struct IpcListener {
    inner: ListenerInner,
}

/// Binds local IPC, rejecting an active listener and remote Windows clients while removing only
/// stale Unix socket files.
pub async fn listen(endpoint: &Endpoint) -> io::Result<IpcListener> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if path.exists() {
                // Probe before unlinking: removing a live socket would orphan its daemon and allow
                // two daemons to control the same backend.
                if tokio::net::UnixStream::connect(path).await.is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!(
                            "another repomond is already listening on {}",
                            path.display()
                        ),
                    ));
                }
                let _ = std::fs::remove_file(path);
            }
            Ok(IpcListener {
                inner: ListenerInner::Unix(tokio::net::UnixListener::bind(path)?),
            })
        }
        #[cfg(windows)]
        Endpoint::Pipe(name) => {
            let security =
                repomon_host::dacl::PipeSecurity::current_user_only().map_err(io::Error::other)?;
            let first = pipe_instance(name, &security, true)?;
            Ok(IpcListener {
                inner: ListenerInner::Pipe {
                    name: name.clone(),
                    security,
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
            ListenerInner::Pipe {
                name,
                security,
                next,
            } => {
                let server = match next.take() {
                    Some(server) => server,
                    // The previous accept failed to pre-create an instance (e.g. a transient
                    // resource error); retry here rather than being wedged forever.
                    None => pipe_instance(name, security, false)?,
                };
                server.connect().await?;
                *next = pipe_instance(name, security, false).ok();
                Ok(IpcStream::PipeServer(server))
            }
        }
    }
}

/// Connects to local IPC, briefly retrying Windows pipe-busy races while returning other errors to
/// the caller.
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

/// Provides an AsyncRead + AsyncWrite local IPC stream, including an in-memory duplex variant for
/// tests.
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
            write_frame(&mut s, &frame).await.unwrap();
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

    /// A second listener must not evict a live Unix socket owner; Windows enforces this through
    /// first_pipe_instance.
    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_to_steal_a_live_listeners_socket() {
        let ep = test_endpoint("live");
        let _first = listen(&ep).await.unwrap(); // still held: this is the "live" listener

        match listen(&ep).await {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::AddrInUse),
            Ok(_) => panic!("expected AddrInUse, got a second listener on the same socket"),
        }
    }

    /// A socket file left behind by a crashed/killed daemon (nothing answers a connect) must
    /// still be reclaimed, so a stale file never permanently blocks the next real daemon start.
    #[cfg(unix)]
    #[tokio::test]
    async fn reclaims_a_stale_socket_file_with_no_live_listener() {
        let ep = test_endpoint("stale");
        {
            // Bind once, then drop without going through `serve`'s graceful-shutdown unlink -
            // `IpcListener` itself has no `Drop` impl that removes the file, so this leaves
            // exactly what a crash leaves: a socket file on disk with nothing listening on it.
            let _dead = listen(&ep).await.unwrap();
        }

        listen(&ep)
            .await
            .expect("a stale, unconnectable socket file must not block a fresh bind");
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
        assert_eq!(
            pipe_name_from_path(Path::new(r"\\.\pipe\repomon-ali")),
            r"\\.\pipe\repomon-ali"
        );

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
