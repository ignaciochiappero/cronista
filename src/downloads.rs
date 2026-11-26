//! Resumable HTTP model file downloader with SHA-256 verification.
//! Exact behavioral parity with `cronista/downloads.py`:
//! - Writes to `<name>.part`, promotes to `<name>` only after exact size & SHA-256 match.
//! - Resumes partial downloads with `Range: bytes=<have>-`.
//! - Deletes corrupted parts; preserves truncated parts on network drop or cancellation.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

pub const DEFAULT_CHUNK: usize = 1 << 20; // 1 MiB

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

impl RemoteFile {
    pub fn new(name: impl Into<String>, url: impl Into<String>, size: u64, sha256: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            size,
            sha256: sha256.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("{0}")]
    Failed(String),
    #[error("Download cancelled")]
    Cancelled,
}

/// Files not yet in `folder` or whose size does not match `file.size`.
pub fn missing(folder: &Path, files: &[RemoteFile]) -> Vec<RemoteFile> {
    files
        .iter()
        .filter(|file| {
            let p = folder.join(&file.name);
            match fs::metadata(&p) {
                Ok(m) if m.is_file() => m.len() != file.size,
                _ => true,
            }
        })
        .cloned()
        .collect()
}

fn sha256_file(path: &Path, chunk_size: usize) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; chunk_size.max(4096)];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn download<P, S>(
    folder: &Path,
    files: &[RemoteFile],
    progress: P,
    stopped: S,
    timeout_secs: u64,
) -> Result<(), DownloadError>
where
    P: FnMut(u64, u64),
    S: Fn() -> bool,
{
    download_with_chunk(folder, files, DEFAULT_CHUNK, progress, stopped, timeout_secs)
}

pub fn download_with_chunk<P, S>(
    folder: &Path,
    files: &[RemoteFile],
    chunk_size: usize,
    mut progress: P,
    stopped: S,
    timeout_secs: u64,
) -> Result<(), DownloadError>
where
    P: FnMut(u64, u64),
    S: Fn() -> bool,
{
    fs::create_dir_all(folder).map_err(|e| {
        DownloadError::Failed(format!(
            "Downloading the model failed ({e}). Check the connection and try again."
        ))
    })?;

    let total: u64 = files.iter().map(|f| f.size).sum();
    let todo = missing(folder, files);
    let todo_bytes: u64 = todo.iter().map(|f| f.size).sum();
    let mut done = total.saturating_sub(todo_bytes);

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(timeout_secs))
        .timeout_read(Duration::from_secs(timeout_secs))
        .build();

    let mut buf = vec![0u8; chunk_size.max(4096)];

    for file in &todo {
        let part = folder.join(format!("{}.part", file.name));
        let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > file.size {
            have = 0;
        }

        let mut req = agent.get(&file.url).set("User-Agent", "Cronista");
        if have > 0 {
            req = req.set("Range", &format!("bytes={have}-"));
        }

        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => {
                return Err(DownloadError::Failed(format!(
                    "Downloading the model failed (HTTP {code}). Check the connection and try again."
                )));
            }
            Err(ureq::Error::Transport(t)) => {
                return Err(DownloadError::Failed(format!(
                    "Downloading the model failed ({t}). Check the connection and try again."
                )));
            }
        };

        if have > 0 && resp.status() != 206 {
            have = 0;
        }

        let mut out = if have > 0 {
            OpenOptions::new().create(true).append(true).open(&part)
        } else {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&part)
        }
        .map_err(|e| {
            DownloadError::Failed(format!(
                "Downloading the model failed ({e}). Check the connection and try again."
            ))
        })?;

        let mut reader = resp.into_reader();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    return Err(DownloadError::Failed(format!(
                        "Downloading the model failed ({e}). Check the connection and try again."
                    )));
                }
            };
            out.write_all(&buf[..n]).map_err(|e| {
                DownloadError::Failed(format!(
                    "Downloading the model failed ({e}). Check the connection and try again."
                ))
            })?;
            have += n as u64;
            progress(done + have, total);
            if stopped() {
                return Err(DownloadError::Cancelled);
            }
        }
        drop(out);

        if have < file.size {
            return Err(DownloadError::Failed(
                "Downloading the model failed (the connection ended early). Check the connection and try again."
                    .to_string(),
            ));
        }
        let actual_sha = sha256_file(&part, chunk_size).map_err(|e| {
            DownloadError::Failed(format!(
                "Downloading the model failed ({e}). Check the connection and try again."
            ))
        })?;
        if have > file.size || (!file.sha256.is_empty() && actual_sha != file.sha256) {
            let _ = fs::remove_file(&part);
            return Err(DownloadError::Failed(
                "The downloaded model is not the expected one. Try again.".to_string(),
            ));
        }

        let dest = folder.join(&file.name);
        if dest.exists() {
            let _ = fs::remove_file(&dest);
        }
        fs::rename(&part, &dest).map_err(|e| {
            DownloadError::Failed(format!(
                "Downloading the model failed ({e}). Check the connection and try again."
            ))
        })?;
        done += file.size;
    }

    progress(total, total);
    Ok(())
}
