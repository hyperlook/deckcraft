//! Host-owned, explicitly selected text material. No paths enter the document.
use std::io::Read;
use std::path::Path;

const MAX_BYTES: u64 = 128_000;
const MAX_CHARS: usize = 32_000;

pub fn pick() -> Result<Option<(String, String)>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("UTF-8 text", &["txt", "md"]).pick_file() else {
        return Ok(None);
    };
    read(&path).map(Some)
}

fn read(path: &Path) -> Result<(String, String), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if !matches!(ext.as_str(), "txt" | "md") {
        return Err("请选择 UTF-8 txt/md 文件".into());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("素材文件超过 128 KB".into());
    }
    let mut bytes = Vec::new();
    // File can grow after metadata inspection; cap the actual read as well.
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("素材文件超过 128 KB".into());
    }
    let body = String::from_utf8(bytes).map_err(|_| "素材必须是 UTF-8 文本".to_string())?;
    if body.chars().count() > MAX_CHARS {
        return Err("素材超过 32000 字符，请先缩短".into());
    }
    let body = body.strip_prefix('\u{feff}').unwrap_or(&body).to_string();
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "笔记".into());
    Ok((name, body))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    #[test]
    fn selection_is_utf8_bounded_and_only_returns_basename_and_content() {
        let dir = std::env::temp_dir().join(format!("deckcraft-material-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        std::fs::write(&path, "\u{feff}非敏感笔记").unwrap();
        assert_eq!(read(&path).unwrap(), ("note.md".into(), "非敏感笔记".into()));
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, "x".repeat(32_001)).unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, vec![b'x'; 128_001]).unwrap();
        assert!(read(&path).is_err());
        assert!(read(&dir.join("file.pdf")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
