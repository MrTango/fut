//! Conservative shell-word recognition, never shell evaluation.
use std::path::{Path, PathBuf};

use anyhow::bail;
use tokio::io::AsyncReadExt;

use crate::protocol::MAX_ATTACHMENT_BYTES;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Only one absolute PNG shell word. Unquoted shell syntax is deliberately
/// rejected, including expansion syntax, even though we never execute a shell.
pub(super) fn parse(text: &str) -> Option<PathBuf> {
    if text.chars().any(char::is_control) {
        return None;
    }
    let mut chars = text.trim_matches(' ').chars();
    let mut word = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => loop {
                match chars.next()? {
                    '\'' => break,
                    ch => word.push(ch),
                }
            },
            '"' => loop {
                match chars.next()? {
                    '"' => break,
                    '$' | '`' => return None,
                    '\\' => {
                        let next = chars.next()?;
                        if !matches!(next, '$' | '`' | '"' | '\\') {
                            word.push('\\');
                        }
                        word.push(next);
                    }
                    ch => word.push(ch),
                }
            },
            '\\' => word.push(chars.next()?),
            ch if ch.is_whitespace() || "|&;<>()$`*?[]{}~#".contains(ch) => return None,
            ch => word.push(ch),
        }
    }
    let path = PathBuf::from(word);
    (path.is_absolute()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png")))
    .then_some(path)
}

/// None means ordinary paste, with its original text untouched. Bound the whole
/// preflight because input is serialized until the fallback decision is known.
pub(super) async fn read(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    match tokio::time::timeout(std::time::Duration::from_secs(2), inspect(path)).await {
        Ok(result) => result,
        Err(_) => Ok(None),
    }
}

async fn inspect(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    // A path can be replaced after metadata. Never block opening a FIFO.
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let Ok(mut file) = options.open(path).await else {
        return Ok(None);
    };
    let Ok(metadata) = file.metadata().await else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let mut signature = [0; 8];
    if file.read_exact(&mut signature).await.is_err() || signature != PNG_SIGNATURE {
        return Ok(None);
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES as u64 {
        bail!("PNG exceeds image upload limit (20 MiB)");
    }
    let mut png = signature.to_vec();
    if file
        .take((MAX_ATTACHMENT_BYTES + 1 - signature.len()) as u64)
        .read_to_end(&mut png)
        .await
        .is_err()
    {
        return Ok(None);
    }
    if png.len() > MAX_ATTACHMENT_BYTES {
        bail!("PNG exceeds image upload limit (20 MiB)");
    }
    Ok(Some(png))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_one_shell_word_without_evaluating_it() {
        for (text, path) in [
            (" /tmp/image.png  ", "/tmp/image.png"),
            ("'/tmp/screen shot.png'", "/tmp/screen shot.png"),
            (r#""/tmp/screen shot.PNG""#, "/tmp/screen shot.PNG"),
            (r"/tmp/screen\ shot.png", "/tmp/screen shot.png"),
            (r"'/tmp/it'\''s.png'", "/tmp/it's.png"),
            (r"/tmp/it\'s.png", "/tmp/it's.png"),
            (r"/tmp/back\\slash.png", r"/tmp/back\slash.png"),
            (r#""/tmp/back\slash.png""#, r"/tmp/back\slash.png"),
            ("'/tmp/$(touch nope).png'", "/tmp/$(touch nope).png"),
        ] {
            assert_eq!(parse(text), Some(PathBuf::from(path)), "{text}");
        }
        for text in [
            "",
            "foo.png",
            "~/foo.png",
            "file:///tmp/foo.png",
            "/tmp/a.jpg",
            "/tmp/a.png /tmp/b.png",
            "/tmp/a.png;whoami",
            "/tmp/$(whoami).png",
            "\"/tmp/$HOME.png\"",
            "/tmp/`whoami`.png",
            "/tmp/*.png",
            "'/tmp/a.png",
            "/tmp/a.png\\",
            "/tmp/a.png\n",
            "/tmp/a\t.png",
            "/tmp/a\0.png",
            "/tmp/a.png\u{1b}",
            "/tmp/a.png && true",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[tokio::test]
    async fn local_preflight_preserves_bytes_or_falls_back_without_changing_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("it's a screenshot.png");
        let text = format!(
            "  {}  ",
            path.display()
                .to_string()
                .replace(' ', "\\ ")
                .replace('\'', "\\'")
        );
        let parsed = parse(&text).unwrap();
        assert_eq!(read(&parsed).await.unwrap(), None); // Missing.
        tokio::fs::create_dir(&path).await.unwrap();
        assert_eq!(read(&parsed).await.unwrap(), None); // Directory.
        tokio::fs::remove_dir(&path).await.unwrap();
        tokio::fs::write(&path, b"not a PNG").await.unwrap();
        assert_eq!(read(&parsed).await.unwrap(), None);
        let png = [PNG_SIGNATURE, b"unchanged image bytes"].concat();
        tokio::fs::write(&path, &png).await.unwrap();
        assert_eq!(read(&parsed).await.unwrap(), Some(png));
    }

    #[tokio::test]
    async fn oversized_png_is_an_error_but_oversized_nonimage_is_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.png");
        tokio::fs::write(&path, PNG_SIGNATURE).await.unwrap();
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .unwrap();
        file.set_len(MAX_ATTACHMENT_BYTES as u64).await.unwrap();
        assert_eq!(
            read(&path).await.unwrap().unwrap().len(),
            MAX_ATTACHMENT_BYTES
        );
        file.set_len(MAX_ATTACHMENT_BYTES as u64 + 1).await.unwrap();
        assert!(
            read(&path)
                .await
                .unwrap_err()
                .to_string()
                .contains("20 MiB")
        );
        tokio::fs::write(&path, b"not an image").await.unwrap();
        file.set_len(MAX_ATTACHMENT_BYTES as u64 + 1).await.unwrap();
        assert_eq!(read(&path).await.unwrap(), None);
    }
}
