//! Connection-scoped, bounded PNG uploads and daemon-owned private files.
use std::{io::Write, path::Path, sync::Arc};

use anyhow::{Result, bail, ensure};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::{
    domain::TerminalId,
    protocol::{ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES},
};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
// Includes pending uploads and retained files. Charging at least one chunk also
// bounds the number of tiny files. Exhaustion fails rather than evicting paths
// that a shell/application may still be using.
const STORE_BYTES: usize = 256 * 1024 * 1024;

pub(crate) struct Store {
    budget: Arc<Semaphore>,
    files: std::sync::Mutex<Vec<ManagedFile>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            budget: Arc::new(Semaphore::new(STORE_BYTES)),
            files: Default::default(),
        }
    }
}

impl Store {
    pub(crate) fn retain(&self, file: ManagedFile) {
        self.files
            .lock()
            .expect("attachment store poisoned")
            .push(file);
    }
}

struct Pending {
    id: Uuid,
    terminal_id: TerminalId,
    size: usize,
    bytes: Vec<u8>,
    permit: OwnedSemaphorePermit,
}

#[derive(Default)]
pub(crate) struct Upload {
    pending: Option<Pending>,
}

impl Upload {
    pub(crate) fn clear(&mut self) {
        self.pending = None;
    }

    pub(crate) fn begin(
        &mut self,
        store: &Store,
        id: Uuid,
        terminal_id: TerminalId,
        size: u64,
    ) -> Result<()> {
        self.clear();
        ensure!(
            (PNG_SIGNATURE.len() as u64..=MAX_ATTACHMENT_BYTES as u64).contains(&size),
            "PNG must be between 8 bytes and 20 MiB"
        );
        let size = size as usize;
        let permit = store.budget.clone().try_acquire_many_owned(size.max(ATTACHMENT_CHUNK_BYTES) as u32)
            .map_err(|_| anyhow::anyhow!("attachment storage quota exhausted; restart the daemon to release retained images"))?;
        self.pending = Some(Pending {
            id,
            terminal_id,
            size,
            bytes: Vec::with_capacity(size),
            permit,
        });
        Ok(())
    }

    pub(crate) fn chunk(&mut self, id: Uuid, bytes: Vec<u8>) -> Result<()> {
        let Some(mut pending) = self.pending.take() else {
            bail!("no pending attachment");
        };
        ensure!(pending.id == id, "attachment transfer ID mismatch");
        ensure!(
            !bytes.is_empty() && bytes.len() <= ATTACHMENT_CHUNK_BYTES,
            "attachment chunk must contain 1 to 64 KiB"
        );
        ensure!(
            bytes.len() <= pending.size - pending.bytes.len(),
            "attachment exceeds declared size"
        );
        pending.bytes.extend_from_slice(&bytes);
        self.pending = Some(pending);
        Ok(())
    }

    pub(crate) fn cancel(&mut self, id: Uuid) -> Result<()> {
        let pending = self.pending.take();
        ensure!(
            pending.is_none_or(|pending| pending.id == id),
            "attachment transfer ID mismatch"
        );
        Ok(())
    }

    pub(crate) async fn commit(&mut self, id: Uuid) -> Result<(TerminalId, ManagedFile)> {
        let pending = self
            .pending
            .take()
            .ok_or_else(|| anyhow::anyhow!("no pending attachment"))?;
        ensure!(pending.id == id, "attachment transfer ID mismatch");
        ensure!(
            pending.bytes.len() == pending.size,
            "attachment size does not match declaration"
        );
        ensure!(
            pending.bytes.starts_with(PNG_SIGNATURE),
            "attachment is not a PNG"
        );
        let terminal_id = pending.terminal_id;
        let file = tokio::task::spawn_blocking(move || -> Result<ManagedFile> {
            // NamedTempFile creates a random, exclusive, mode-0600 file. No
            // peer-supplied path or filename ever reaches the filesystem.
            let directory = std::fs::canonicalize(std::env::temp_dir())?;
            let file = tempfile::Builder::new()
                .prefix("fut-attachment-")
                .suffix(".png")
                .tempfile_in(directory)?;
            let mut managed = ManagedFile {
                file: Some(file),
                permit: Some(pending.permit),
            };
            let file = managed.file.as_mut().unwrap();
            file.write_all(&pending.bytes)?;
            file.flush()?;
            Ok(managed)
        })
        .await??;
        Ok((terminal_id, file))
    }
}

pub(crate) struct ManagedFile {
    file: Option<tempfile::NamedTempFile>,
    permit: Option<OwnedSemaphorePermit>,
}

impl ManagedFile {
    pub(crate) fn path(&self) -> &Path {
        self.file.as_ref().unwrap().path()
    }

    pub(crate) fn quoted_path(&self) -> Result<String> {
        let path = self
            .path()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("attachment path is not UTF-8"))?;
        // Reject control characters: terminal bracketed-paste delimiters must
        // not be injectable even through a hostile TMPDIR environment.
        ensure!(
            !path.chars().any(char::is_control),
            "attachment path contains control characters"
        );
        Ok(format!("'{}'", path.replace('\'', "'\\''")))
    }
}

impl Drop for ManagedFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let permit = self.permit.take();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn_blocking(move || drop((file, permit)));
            } else {
                drop((file, permit));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attachment_pins_target_and_persists_private_png() {
        use std::os::unix::fs::PermissionsExt;
        let store = Store::default();
        let mut upload = Upload::default();
        let id = Uuid::new_v4();
        let terminal_id = TerminalId::new();
        upload.begin(&store, id, terminal_id, 8).unwrap();
        upload.chunk(id, PNG_SIGNATURE[..3].to_vec()).unwrap();
        upload.chunk(id, PNG_SIGNATURE[3..].to_vec()).unwrap();
        let (target, file) = upload.commit(id).await.unwrap();
        assert_eq!(target, terminal_id);
        assert_eq!(tokio::fs::read(file.path()).await.unwrap(), PNG_SIGNATURE);
        assert_eq!(
            tokio::fs::metadata(file.path())
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(file.quoted_path().unwrap().starts_with('\''));
        let path = file.path().to_owned();
        store.retain(file);
        assert!(path.exists());
        drop(store);
        for _ in 0..100 {
            if !path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("managed file was not cleaned up");
    }

    #[tokio::test]
    async fn attachment_accepts_exact_limits_and_cleans_unretained_commit() {
        let store = Store::default();
        let mut upload = Upload::default();
        let id = Uuid::new_v4();
        upload
            .begin(&store, id, TerminalId::new(), MAX_ATTACHMENT_BYTES as u64)
            .unwrap();
        let mut chunk = vec![0; ATTACHMENT_CHUNK_BYTES];
        chunk[..8].copy_from_slice(PNG_SIGNATURE);
        upload.chunk(id, chunk).unwrap();
        for _ in 1..MAX_ATTACHMENT_BYTES / ATTACHMENT_CHUNK_BYTES {
            upload.chunk(id, vec![0; ATTACHMENT_CHUNK_BYTES]).unwrap();
        }
        let (_, file) = upload.commit(id).await.unwrap();
        assert_eq!(
            tokio::fs::metadata(file.path()).await.unwrap().len(),
            MAX_ATTACHMENT_BYTES as u64
        );
        let path = file.path().to_owned();
        // A failed terminal paste does not retain the committed file.
        drop(file);
        for _ in 0..100 {
            if store.budget.available_permits() == STORE_BYTES {
                assert!(!path.exists());
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("unretained attachment was not cleaned up");
    }

    #[tokio::test]
    async fn attachment_bounds_and_failures_release_pending_budget() {
        let store = Store::default();
        let mut upload = Upload::default();
        let id = Uuid::new_v4();
        let target = TerminalId::new();
        for size in [0, 7, MAX_ATTACHMENT_BYTES as u64 + 1, u64::MAX] {
            assert!(upload.begin(&store, id, target, size).is_err());
            assert!(upload.pending.is_none());
        }
        upload
            .begin(&store, id, target, MAX_ATTACHMENT_BYTES as u64)
            .unwrap();
        upload.chunk(id, vec![0; ATTACHMENT_CHUNK_BYTES]).unwrap();
        assert!(
            upload
                .chunk(id, vec![0; ATTACHMENT_CHUNK_BYTES + 1])
                .is_err()
        );
        assert!(upload.pending.is_none());
        assert_eq!(store.budget.available_permits(), STORE_BYTES);
        upload.begin(&store, id, target, 8).unwrap();
        assert!(upload.chunk(id, vec![0; 9]).is_err());
        upload.begin(&store, id, target, 8).unwrap();
        assert!(upload.chunk(Uuid::new_v4(), vec![0]).is_err());
        upload.begin(&store, id, target, 8).unwrap();
        assert!(upload.commit(id).await.is_err());
        upload.begin(&store, id, target, 8).unwrap();
        upload.chunk(id, vec![0; 8]).unwrap();
        assert!(upload.commit(id).await.is_err());
        assert_eq!(store.budget.available_permits(), STORE_BYTES);
    }

    #[test]
    fn attachment_path_is_shell_quoted_and_rejects_terminal_controls() {
        let store = Store::default();
        for prefix in ["fut-' $()` ", "fut-\n"] {
            let file = ManagedFile {
                file: Some(
                    tempfile::Builder::new()
                        .prefix(prefix)
                        .suffix(".png")
                        .tempfile()
                        .unwrap(),
                ),
                permit: Some(store.budget.clone().try_acquire_owned().unwrap()),
            };
            if prefix.contains('\n') {
                assert!(file.quoted_path().is_err());
            } else {
                let output = std::process::Command::new("/bin/sh")
                    .args(["-c", &format!("printf %s {}", file.quoted_path().unwrap())])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(
                    String::from_utf8(output.stdout).unwrap(),
                    file.path().to_str().unwrap()
                );
            }
        }
    }

    #[test]
    fn attachment_cancel_disconnect_replacement_and_quota() {
        let store = Store::default();
        let mut upload = Upload::default();
        let id = Uuid::new_v4();
        let target = TerminalId::new();
        upload.begin(&store, id, target, 8).unwrap();
        upload.begin(&store, id, target, 8).unwrap();
        assert_eq!(
            store.budget.available_permits(),
            STORE_BYTES - ATTACHMENT_CHUNK_BYTES
        );
        upload.cancel(id).unwrap();
        assert_eq!(store.budget.available_permits(), STORE_BYTES);
        upload.begin(&store, id, target, 8).unwrap();
        drop(upload);
        assert_eq!(store.budget.available_permits(), STORE_BYTES);
        let _all = store
            .budget
            .clone()
            .try_acquire_many_owned(STORE_BYTES as u32)
            .unwrap();
        assert!(Upload::default().begin(&store, id, target, 8).is_err());
    }
}
