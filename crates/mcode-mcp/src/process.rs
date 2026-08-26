//! Direct-spawn stdio and process-containment adapter contracts.

// Rust guideline compliant 2026-08-20.

use std::{collections::BTreeMap, fmt, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

use async_trait::async_trait;
use rmcp::{
    RoleClient,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Mutex,
};

use crate::{
    error::{Error, ErrorKind, Recovery, Result},
    identity::ServerName,
    secret::SecretValue,
};

/// Boxed asynchronous child stdout/stderr reader.
pub type BoxAsyncRead = Pin<Box<dyn AsyncRead + Send + 'static>>;
/// Boxed asynchronous child stdin writer.
pub type BoxAsyncWrite = Pin<Box<dyn AsyncWrite + Send + 'static>>;

/// Fully materialized direct-spawn request.
///
/// A process host must not concatenate these fields into a shell command.
pub struct ProcessSpec {
    /// Server provenance.
    pub server: ServerName,
    /// Executable passed directly to an operating-system process API.
    pub executable: String,
    /// Exact argument vector.
    pub args: Vec<String>,
    /// Optional child working directory.
    pub cwd: Option<PathBuf>,
    /// Parent environment names explicitly allowed to be copied.
    pub inherit_env: Vec<String>,
    /// Secret environment values resolved by the authorized host.
    pub secret_env: BTreeMap<String, SecretValue>,
}

impl fmt::Debug for ProcessSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessSpec")
            .field("server", &self.server)
            .field("executable", &self.executable)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("inherit_env", &self.inherit_env)
            .field("secret_env", &self.secret_env.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Observed child exit status without platform-specific handle leakage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessExit {
    /// Whether the process reported success.
    pub success: bool,
    /// Optional numeric exit code.
    pub code: Option<i32>,
}

/// Ownership interface for a contained process tree.
///
/// The plugin-host implementation is responsible for MCode's Job Object on
/// Windows or dedicated PGID on Unix. Dropping an implementation must also be
/// kill-safe so an aborted future cannot orphan descendants.
#[async_trait]
pub trait ContainedProcess: Send + 'static {
    /// Waits for the process leader to exit and reaps it.
    async fn wait(&mut self) -> std::io::Result<ProcessExit>;

    /// Terminates the complete Job/PGID process tree.
    async fn kill_tree(&mut self) -> std::io::Result<()>;
}

/// A direct child with owned stdio and containment control.
pub struct SpawnedProcess {
    /// Child stdout carrying MCP JSON-RPC.
    pub stdout: BoxAsyncRead,
    /// Child stdin carrying MCP JSON-RPC.
    pub stdin: BoxAsyncWrite,
    /// Optional stderr stream; never interpreted as protocol data.
    pub stderr: Option<BoxAsyncRead>,
    /// Process-tree owner.
    pub process: Box<dyn ContainedProcess>,
}

impl fmt::Debug for SpawnedProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SpawnedProcess(..)")
    }
}

/// Host adapter that directly spawns contained stdio servers.
#[async_trait]
pub trait ProcessHost: Send + Sync + 'static {
    /// Spawns exactly `spec.executable` with `spec.args`, without a shell.
    async fn spawn_direct(&self, spec: ProcessSpec) -> Result<SpawnedProcess>;
}

/// Cloneable, type-erased process host.
#[derive(Clone)]
pub struct ProcessHostHandle(Arc<dyn ProcessHost>);

impl ProcessHostHandle {
    /// Erases a concrete process host.
    #[must_use]
    pub fn new(host: impl ProcessHost) -> Self {
        Self(Arc::new(host))
    }

    pub(crate) async fn spawn_direct(&self, spec: ProcessSpec) -> Result<SpawnedProcess> {
        self.0.spawn_direct(spec).await
    }
}

impl fmt::Debug for ProcessHostHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProcessHostHandle(..)")
    }
}

/// Process host that fails explicitly until the plugin host supplies containment.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProcessHost;

#[async_trait]
impl ProcessHost for NoProcessHost {
    async fn spawn_direct(&self, spec: ProcessSpec) -> Result<SpawnedProcess> {
        Err(Error::new(
            ErrorKind::Unavailable,
            Recovery::Fatal,
            "stdio transport requires a direct-spawn Job/PGID ProcessHost adapter",
        )
        .with_server(spec.server))
    }
}

/// Errors produced by the bounded stdio transport.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StdioTransportError {
    /// Child pipe I/O failed.
    #[error("stdio I/O failed")]
    Io(#[source] std::io::Error),
    /// A wire frame exceeded the configured cap.
    #[error("stdio MCP frame exceeded its configured cap")]
    FrameTooLarge,
    /// A wire frame was not a valid server JSON-RPC message.
    #[error("stdio MCP frame was invalid")]
    InvalidFrame,
    /// Graceful process shutdown exceeded its deadline.
    #[error("contained process shutdown timed out")]
    ShutdownTimeout,
}

impl From<std::io::Error> for StdioTransportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Bounded newline-delimited transport retaining the contained process owner.
pub struct BoundedStdioTransport {
    reader: BufReader<BoxAsyncRead>,
    writer: Arc<Mutex<Option<BoxAsyncWrite>>>,
    process: Option<Box<dyn ContainedProcess>>,
    max_frame_bytes: usize,
    shutdown_timeout: Duration,
}

impl fmt::Debug for BoundedStdioTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedStdioTransport")
            .field("max_frame_bytes", &self.max_frame_bytes)
            .field("shutdown_timeout", &self.shutdown_timeout)
            .finish_non_exhaustive()
    }
}

impl BoundedStdioTransport {
    /// Wraps a contained process in bounded MCP framing.
    #[must_use]
    pub fn new(
        process: SpawnedProcess,
        max_frame_bytes: usize,
        shutdown_timeout: Duration,
    ) -> (Self, Option<BoxAsyncRead>) {
        let SpawnedProcess {
            stdout,
            stdin,
            stderr,
            process,
        } = process;
        (
            Self {
                reader: BufReader::new(stdout),
                writer: Arc::new(Mutex::new(Some(stdin))),
                process: Some(process),
                max_frame_bytes,
                shutdown_timeout,
            },
            stderr,
        )
    }

    async fn receive_frame(&mut self) -> std::result::Result<Option<Vec<u8>>, StdioTransportError> {
        let read_limit = self.max_frame_bytes.saturating_add(2);
        let mut frame = Vec::with_capacity(read_limit.min(64 * 1024));
        let count = {
            let mut limited = (&mut self.reader).take(read_limit as u64);
            limited.read_until(b'\n', &mut frame).await?
        };
        if count == 0 {
            return Ok(None);
        }
        let terminated = frame.last() == Some(&b'\n');
        if !terminated {
            self.discard_line_tail().await?;
            return Err(StdioTransportError::FrameTooLarge);
        }
        let _ = frame.pop();
        if frame.last() == Some(&b'\r') {
            let _ = frame.pop();
        }
        if frame.len() > self.max_frame_bytes {
            return Err(StdioTransportError::FrameTooLarge);
        }
        Ok(Some(frame))
    }

    async fn discard_line_tail(&mut self) -> std::io::Result<()> {
        loop {
            let buffer = self.reader.fill_buf().await?;
            if buffer.is_empty() {
                return Ok(());
            }
            if let Some(offset) = buffer.iter().position(|byte| *byte == b'\n') {
                self.reader.consume(offset + 1);
                return Ok(());
            }
            let length = buffer.len();
            self.reader.consume(length);
        }
    }

    async fn close_process(&mut self) -> std::result::Result<(), StdioTransportError> {
        let mut writer = self.writer.lock().await;
        if let Some(mut stdin) = writer.take() {
            let _ = stdin.shutdown().await;
        }
        drop(writer);

        let Some(process) = self.process.as_mut() else {
            return Ok(());
        };
        match tokio::time::timeout(self.shutdown_timeout, process.wait()).await {
            Ok(result) => {
                let _ = result?;
                self.process = None;
                Ok(())
            }
            Err(_) => {
                process.kill_tree().await?;
                match tokio::time::timeout(self.shutdown_timeout, process.wait()).await {
                    Ok(result) => {
                        let _ = result?;
                        self.process = None;
                        Ok(())
                    }
                    Err(_) => Err(StdioTransportError::ShutdownTimeout),
                }
            }
        }
    }
}

impl Transport<RoleClient> for BoundedStdioTransport {
    type Error = StdioTransportError;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = std::result::Result<(), Self::Error>> + Send + 'static {
        let writer = Arc::clone(&self.writer);
        let max_frame_bytes = self.max_frame_bytes;
        async move {
            let mut frame =
                serde_json::to_vec(&item).map_err(|_| StdioTransportError::InvalidFrame)?;
            if frame.len() > max_frame_bytes {
                return Err(StdioTransportError::FrameTooLarge);
            }
            frame.push(b'\n');
            let mut writer = writer.lock().await;
            let Some(writer) = writer.as_mut() else {
                return Err(StdioTransportError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "stdio transport is closed",
                )));
            };
            writer.write_all(&frame).await?;
            writer.flush().await?;
            Ok(())
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        loop {
            let frame = match self.receive_frame().await {
                Ok(Some(frame)) => frame,
                Ok(None) => return None,
                Err(error) => {
                    tracing::warn!(error.type = ?error, "bounded stdio transport rejected a frame");
                    return None;
                }
            };
            if frame.is_empty() {
                continue;
            }
            match serde_json::from_slice::<ServerJsonRpcMessage>(&frame) {
                Ok(message) => return Some(message),
                Err(_) => {
                    tracing::warn!("bounded stdio transport rejected invalid JSON-RPC");
                    return None;
                }
            }
        }
    }

    async fn close(&mut self) -> std::result::Result<(), Self::Error> {
        self.close_process().await
    }
}

// Keep these aliases checked against rmcp's role-specific transport contract.
const _: fn(ClientJsonRpcMessage) = |_| {};

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use rmcp::model::{EmptyObject, RequestId, ServerResult};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    use super::*;

    struct FakeProcess {
        waits: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ContainedProcess for FakeProcess {
        async fn wait(&mut self) -> std::io::Result<ProcessExit> {
            self.waits.fetch_add(1, Ordering::SeqCst);
            Ok(ProcessExit {
                success: true,
                code: Some(0),
            })
        }

        async fn kill_tree(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn in_memory_stdio_frames_and_reaps_on_close() {
        let (transport_stdout, mut server_stdout) = tokio::io::duplex(4_096);
        let (mut server_stdin, transport_stdin) = tokio::io::duplex(4_096);
        let waits = Arc::new(AtomicUsize::new(0));
        let process = SpawnedProcess {
            stdout: Box::pin(transport_stdout),
            stdin: Box::pin(transport_stdin),
            stderr: None,
            process: Box::new(FakeProcess {
                waits: Arc::clone(&waits),
            }),
        };
        let (mut transport, _) =
            BoundedStdioTransport::new(process, 1_024, Duration::from_millis(50));

        let incoming = ServerJsonRpcMessage::response(
            ServerResult::EmptyResult(EmptyObject {}),
            RequestId::Number(1),
        );
        let mut encoded = serde_json::to_vec(&incoming).unwrap();
        encoded.push(b'\n');
        server_stdout.write_all(&encoded).await.unwrap();
        assert!(transport.receive().await.is_some());

        let outgoing = ClientJsonRpcMessage::request(
            rmcp::model::ClientRequest::PingRequest(rmcp::model::PingRequest::default()),
            RequestId::Number(2),
        );
        transport.send(outgoing).await.unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(&mut server_stdin);
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("\"method\":\"ping\""));

        transport.close().await.unwrap();
        assert_eq!(waits.load(Ordering::SeqCst), 1);
    }

    struct HangingProcess {
        killed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ContainedProcess for HangingProcess {
        async fn wait(&mut self) -> std::io::Result<ProcessExit> {
            while !self.killed.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Ok(ProcessExit {
                success: false,
                code: None,
            })
        }

        async fn kill_tree(&mut self) -> std::io::Result<()> {
            self.killed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn shutdown_kills_the_contained_tree_after_grace_period() {
        let (transport_stdout, _server_stdout) = tokio::io::duplex(64);
        let (_server_stdin, transport_stdin) = tokio::io::duplex(64);
        let killed = Arc::new(AtomicBool::new(false));
        let process = SpawnedProcess {
            stdout: Box::pin(transport_stdout),
            stdin: Box::pin(transport_stdin),
            stderr: None,
            process: Box::new(HangingProcess {
                killed: Arc::clone(&killed),
            }),
        };
        let (mut transport, _) = BoundedStdioTransport::new(process, 64, Duration::from_millis(10));
        transport.close().await.unwrap();
        assert!(killed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn oversized_stdio_frame_closes_without_unbounded_allocation() {
        let (transport_stdout, mut server_stdout) = tokio::io::duplex(512);
        let (_server_stdin, transport_stdin) = tokio::io::duplex(64);
        let process = SpawnedProcess {
            stdout: Box::pin(transport_stdout),
            stdin: Box::pin(transport_stdin),
            stderr: None,
            process: Box::new(FakeProcess {
                waits: Arc::new(AtomicUsize::new(0)),
            }),
        };
        let (mut transport, _) = BoundedStdioTransport::new(process, 32, Duration::from_millis(10));
        server_stdout.write_all(&[b'x'; 128]).await.unwrap();
        server_stdout.write_all(b"\n").await.unwrap();
        assert!(transport.receive().await.is_none());
    }
}
