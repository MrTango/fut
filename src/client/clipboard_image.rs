//! Local clipboard acquisition and a bounded, connection-scoped upload queue.
use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, bail};
use tokio::{io::AsyncReadExt, sync::mpsc, task::JoinHandle};
use uuid::Uuid;

use crate::{
    domain::TerminalId,
    protocol::{ATTACHMENT_CHUNK_BYTES, ClientMessage, MAX_ATTACHMENT_BYTES},
};

const TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_BYTES: usize = 4096;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

pub(super) enum Update {
    Message(ClientMessage),
    Finished(anyhow::Result<()>),
}

#[derive(Default)]
pub(super) struct Upload {
    pending: Option<Pending>,
}

struct Pending {
    updates: mpsc::Receiver<Update>,
    task: JoinHandle<()>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Upload {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn reject_error(&mut self, request_id: Option<Uuid>, code: &str) -> bool {
        if request_id.is_some() || code != "attachment_error" {
            return false;
        }
        // The daemon clears failed transfers before replying. Drop queued chunks
        // too; a cancel here would only produce another error for that dead transfer.
        self.pending = None;
        true
    }

    pub(super) fn start(&mut self, terminal_id: TerminalId) -> anyhow::Result<()> {
        self.start_reading(terminal_id, read_clipboard())
    }

    pub(super) fn start_png(
        &mut self,
        terminal_id: TerminalId,
        png: Vec<u8>,
    ) -> anyhow::Result<()> {
        self.start_reading(terminal_id, async move { Ok(png) })
    }

    fn start_reading(
        &mut self,
        terminal_id: TerminalId,
        read: impl std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send + 'static,
    ) -> anyhow::Result<()> {
        if self.pending.is_some() {
            bail!("an image paste is already in progress");
        }
        let (updates, receiver) = mpsc::channel(2);
        let task = tokio::spawn(async move {
            let result = async {
                let png = read.await?;
                enqueue(&updates, terminal_id, png).await
            }
            .await;
            let _ = updates.send(Update::Finished(result)).await;
        });
        self.pending = Some(Pending {
            updates: receiver,
            task,
        });
        Ok(())
    }

    pub(super) async fn next(&mut self) -> Update {
        let Some(pending) = self.pending.as_mut() else {
            return std::future::pending().await;
        };
        let update = pending.updates.recv().await.unwrap_or_else(|| {
            Update::Finished(Err(anyhow::anyhow!("image paste worker stopped")))
        });
        if matches!(update, Update::Finished(_)) {
            self.pending = None;
        }
        update
    }
}

async fn enqueue(
    updates: &mpsc::Sender<Update>,
    terminal_id: TerminalId,
    png: Vec<u8>,
) -> anyhow::Result<()> {
    let transfer_id = Uuid::new_v4();
    let send = async |message| {
        updates
            .send(Update::Message(message))
            .await
            .context("image paste connection closed")
    };
    send(ClientMessage::AttachmentBegin {
        transfer_id,
        terminal_id,
        size: png.len() as u64,
    })
    .await?;
    for chunk in png.chunks(ATTACHMENT_CHUNK_BYTES) {
        send(ClientMessage::AttachmentChunk {
            transfer_id,
            bytes: chunk.to_vec(),
        })
        .await?;
    }
    send(ClientMessage::AttachmentCommit { transfer_id }).await
}

// AppKit provides both native PNG and TIFF (common for macOS screenshots).
// Write binary NSData directly: osascript's normal result printing corrupts PNG.
#[cfg(target_os = "macos")]
const MAC_SCRIPT: &str = r#"
ObjC.import('AppKit');
ObjC.import('Foundation');
var pb = $.NSPasteboard.generalPasteboard;
var png = pb.dataForType('public.png');
if (!(Number(png.length) > 0)) {
    var tiff = pb.dataForType('public.tiff');
    if (!(Number(tiff.length) > 0)) throw new Error('clipboard does not contain an image');
    var bitmap = $.NSBitmapImageRep.imageRepWithData(tiff);
    png = bitmap.representationUsingTypeProperties($.NSPNGFileType, $({}));
}
if (!(Number(png.length) > 0)) throw new Error('clipboard image could not be converted to PNG');
$.NSFileHandle.fileHandleWithStandardOutput.writeData(png);
"#;

fn commands(wayland: bool, x11: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    #[cfg(target_os = "macos")]
    {
        let _ = (wayland, x11);
        vec![(
            "/usr/bin/osascript",
            vec!["-l", "JavaScript", "-e", MAC_SCRIPT],
        )]
    }
    #[cfg(target_os = "linux")]
    {
        let mut commands = Vec::new();
        if wayland {
            commands.push(("wl-paste", vec!["--no-newline", "--type", "image/png"]));
        }
        if x11 {
            commands.push((
                "xclip",
                vec!["-selection", "clipboard", "-t", "image/png", "-o"],
            ));
        }
        commands
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (wayland, x11);
        Vec::new()
    }
}

async fn read_clipboard() -> anyhow::Result<Vec<u8>> {
    let commands = commands(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var_os("DISPLAY").is_some(),
    );
    if commands.is_empty() {
        bail!("image clipboard unavailable: no supported graphical session");
    }
    let mut failures = Vec::new();
    for (program, args) in commands {
        match read_process(Path::new(program), &args, TIMEOUT, MAX_ATTACHMENT_BYTES).await {
            Ok(png) => return Ok(png),
            Err(error) => failures.push(format!("{program}: {error:#}")),
        }
    }
    bail!(
        "could not read clipboard PNG: {}{}",
        if cfg!(target_os = "linux") {
            "install wl-paste (Wayland) or xclip (X11); "
        } else {
            ""
        },
        failures.join("; ")
    )
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    max: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(max as u64 + 1).read_to_end(&mut bytes).await?;
    if bytes.len() > max {
        bail!("clipboard process output exceeds limit ({max} bytes)");
    }
    Ok(bytes)
}

async fn read_process(
    program: &Path,
    args: &[&str],
    deadline: Duration,
    max: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("start {}", program.display()))?;
    let stdout = child.stdout.take().context("open clipboard stdout")?;
    let stderr = child.stderr.take().context("open clipboard stderr")?;
    let operation = async {
        let (png, stderr, status) = tokio::try_join!(
            read_bounded(stdout, max),
            read_bounded(stderr, STDERR_BYTES),
            async { child.wait().await.context("wait for clipboard process") },
        )?;
        if !status.success() {
            bail!(
                "clipboard process exited with {status}: {}",
                String::from_utf8_lossy(&stderr)
            );
        }
        if !png.starts_with(PNG_SIGNATURE) {
            bail!("clipboard does not contain a PNG image");
        }
        Ok(png)
    };
    let result = match tokio::time::timeout(deadline, operation).await {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("clipboard process timed out")),
    };
    if result.is_err() {
        super::kill_and_reap(&mut child).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn native_macos_reader_preserves_png_and_converts_tiff_without_touching_clipboard() {
        use base64::Engine;
        let mut fixture = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut fixture, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 0, 0, 255]).unwrap();
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&fixture);
        for tiff in [false, true] {
            let mock = format!(
                "var fixture = $.NSData.alloc.initWithBase64EncodedStringOptions('{encoded}', 0);\n\
                 var pb = {{ dataForType: function(type) {{\n\
                   if ({tiff}) return type === 'public.tiff' ? $.NSBitmapImageRep.imageRepWithData(fixture).TIFFRepresentation : $.NSData.data;\n\
                   return type === 'public.png' ? fixture : $.NSData.data;\n\
                 }} }};"
            );
            let script = MAC_SCRIPT.replace("var pb = $.NSPasteboard.generalPasteboard;", &mock);
            let output = read_process(
                Path::new("/usr/bin/osascript"),
                &["-l", "JavaScript", "-e", &script],
                TIMEOUT,
                MAX_ATTACHMENT_BYTES,
            )
            .await
            .unwrap();
            if !tiff {
                assert_eq!(output, fixture);
            }
            let mut reader = png::Decoder::new(std::io::Cursor::new(output))
                .read_info()
                .unwrap();
            let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
            let frame = reader.next_frame(&mut pixels).unwrap();
            assert_eq!((frame.width, frame.height), (1, 1));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_commands_request_only_png_from_the_available_session() {
        assert!(commands(false, false).is_empty());
        assert_eq!(
            commands(true, false),
            vec![("wl-paste", vec!["--no-newline", "--type", "image/png"])]
        );
        assert_eq!(
            commands(false, true),
            vec![(
                "xclip",
                vec!["-selection", "clipboard", "-t", "image/png", "-o"]
            )]
        );
        assert_eq!(commands(true, true).len(), 2);
    }

    async fn shell(script: &str, max: usize) -> anyhow::Result<Vec<u8>> {
        read_process(
            Path::new("/bin/sh"),
            &["-c", script],
            Duration::from_millis(200),
            max,
        )
        .await
    }

    #[tokio::test]
    async fn clipboard_process_rejects_empty_text_and_nonzero_status() {
        assert!(
            shell("exit 0", 100)
                .await
                .unwrap_err()
                .to_string()
                .contains("PNG")
        );
        assert!(shell("printf text", 100).await.is_err());
        let error = shell("printf unavailable >&2; exit 7", 100)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unavailable"));
        assert!(error.contains('7'));
        assert!(
            read_process(Path::new("/nonexistent/fut-clipboard"), &[], TIMEOUT, 100)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn clipboard_process_bounds_stdout_stderr_and_time() {
        assert!(
            shell("while :; do printf xxxxxxxxxx; done", 100)
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert!(
            shell("while :; do printf xxxxxxxxxx >&2; done", 100)
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert!(
            shell("exec sleep 10", 100)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }

    #[tokio::test]
    async fn png_bytes_are_preserved_and_size_boundary_is_inclusive() {
        let script = "printf '\\211PNG\\r\\n\\032\\n'";
        assert_eq!(shell(script, 8).await.unwrap(), PNG_SIGNATURE);
        assert!(shell(script, 7).await.is_err());
    }

    #[tokio::test]
    async fn upload_is_ordered_chunked_and_pinned() {
        let terminal_id = TerminalId::new();
        let png = vec![42; ATTACHMENT_CHUNK_BYTES + 1];
        let (sender, mut receiver) = mpsc::channel(2);
        let task = tokio::spawn(enqueue_owned(sender, terminal_id, png));
        let Some(Update::Message(ClientMessage::AttachmentBegin {
            transfer_id,
            terminal_id: pinned,
            size,
        })) = receiver.recv().await
        else {
            panic!("expected begin")
        };
        assert_eq!(pinned, terminal_id);
        assert_eq!(size, (ATTACHMENT_CHUNK_BYTES + 1) as u64);
        for length in [ATTACHMENT_CHUNK_BYTES, 1] {
            let Some(Update::Message(ClientMessage::AttachmentChunk {
                transfer_id: id,
                bytes,
            })) = receiver.recv().await
            else {
                panic!("expected chunk")
            };
            assert_eq!(id, transfer_id);
            assert_eq!(bytes, vec![42; length]);
        }
        assert!(
            matches!(receiver.recv().await, Some(Update::Message(ClientMessage::AttachmentCommit { transfer_id: id })) if id == transfer_id)
        );
        task.await.unwrap().unwrap();
    }

    async fn enqueue_owned(
        sender: mpsc::Sender<Update>,
        terminal: TerminalId,
        png: Vec<u8>,
    ) -> anyhow::Result<()> {
        enqueue(&sender, terminal, png).await
    }

    #[tokio::test]
    async fn local_path_bytes_use_pinned_queue_without_overwriting_clipboard_upload() {
        let original = TerminalId::new();
        let other = TerminalId::new();
        let mut upload = Upload::default();
        upload.start_png(original, PNG_SIGNATURE.to_vec()).unwrap();
        assert!(upload.start_png(other, vec![42]).is_err());
        assert!(upload.start(other).is_err());
        assert!(matches!(upload.next().await,
            Update::Message(ClientMessage::AttachmentBegin { terminal_id, .. }) if terminal_id == original));
        assert!(matches!(upload.next().await,
            Update::Message(ClientMessage::AttachmentChunk { bytes, .. }) if bytes == PNG_SIGNATURE));
        assert!(matches!(
            upload.next().await,
            Update::Message(ClientMessage::AttachmentCommit { .. })
        ));
        assert!(matches!(upload.next().await, Update::Finished(Ok(()))));
        assert!(!upload.is_pending());
        upload.start_reading(other, std::future::pending()).unwrap();
        assert!(upload.start_png(original, PNG_SIGNATURE.to_vec()).is_err());
    }

    #[tokio::test]
    async fn asynchronous_read_pins_original_target_and_failure_clears_busy_state() {
        let original = TerminalId::new();
        let mut upload = Upload::default();
        let (ready, clipboard) = tokio::sync::oneshot::channel();
        upload
            .start_reading(original, async { Ok(clipboard.await?) })
            .unwrap();
        let next_focused = TerminalId::new();
        assert!(upload.start(next_focused).is_err());
        ready.send(PNG_SIGNATURE.to_vec()).unwrap();
        assert!(
            matches!(upload.next().await, Update::Message(ClientMessage::AttachmentBegin { terminal_id, .. }) if terminal_id == original)
        );
        assert!(matches!(
            upload.next().await,
            Update::Message(ClientMessage::AttachmentChunk { .. })
        ));
        assert!(matches!(
            upload.next().await,
            Update::Message(ClientMessage::AttachmentCommit { .. })
        ));
        assert!(matches!(upload.next().await, Update::Finished(Ok(()))));
        assert!(!upload.is_pending());

        upload
            .start_reading(next_focused, async { bail!("no clipboard image") })
            .unwrap();
        assert!(
            matches!(upload.next().await, Update::Finished(Err(error)) if error.to_string() == "no clipboard image")
        );
        assert!(!upload.is_pending());
    }

    #[tokio::test]
    async fn repeated_invocation_is_rejected_and_dropping_upload_aborts_worker() {
        let (sender, receiver) = mpsc::channel(2);
        let task = tokio::spawn(async move { sender.closed().await });
        let abort = task.abort_handle();
        let mut upload = Upload {
            pending: Some(Pending {
                updates: receiver,
                task,
            }),
        };
        assert!(
            upload
                .start(TerminalId::new())
                .unwrap_err()
                .to_string()
                .contains("already")
        );
        drop(upload);
        tokio::task::yield_now().await;
        assert!(abort.is_finished());
    }

    #[tokio::test]
    async fn server_rejection_discards_queued_upload_and_recognizes_late_commit_errors() {
        let mut upload = Upload::default();
        upload
            .start_reading(TerminalId::new(), std::future::pending())
            .unwrap();
        assert!(!upload.reject_error(Some(Uuid::new_v4()), "not_found"));
        assert!(upload.is_pending());
        assert!(!upload.reject_error(None, "unrelated_error"));
        assert!(upload.is_pending());
        assert!(upload.reject_error(None, "attachment_error"));
        assert!(!upload.is_pending());
        assert!(upload.reject_error(None, "attachment_error"));
        assert!(!upload.reject_error(None, "unrelated_error"));
    }
}
