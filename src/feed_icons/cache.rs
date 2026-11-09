// SPDX-License-Identifier: Apache-2.0
//! Small PNG thumbnails, with a week-long successful cache and a one-day miss backoff.
use super::discovery::Source;
use serde::{Deserialize, Serialize};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Entry {
    source: Source,
    checked: i64,
    image: Option<String>,
}

pub struct Cached {
    pub path: Option<String>,
    pub fresh: bool,
}

pub fn key(source: &Source) -> u64 {
    daynews_db::feed_id(&serde_json::to_string(source).expect("serializable icon source"))
}

pub fn read(root: &Path, source: &Source, now: i64) -> Cached {
    let empty = || Cached {
        path: None,
        fresh: false,
    };
    let Some(entry) = std::fs::read(root.join(format!("{:x}.json", key(source))))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok())
    else {
        return empty();
    };
    if entry.source != *source {
        return empty();
    }
    let path = entry
        .image
        .as_deref()
        .filter(|name| !name.contains(['/', '\\']) && name.ends_with(".png"))
        .map(|name| root.join(name))
        .filter(|path| path.is_file());
    let missing = entry.image.is_some() && path.is_none();
    let age = now.saturating_sub(entry.checked);
    Cached {
        path: path.map(|p| p.to_string_lossy().into_owned()),
        fresh: !missing
            && (0..if entry.image.is_some() {
                7 * 86400
            } else {
                86400
            })
                .contains(&age),
    }
}

pub fn write(root: &Path, source: &Source, png: Option<Vec<u8>>, now: i64) -> Option<String> {
    std::fs::create_dir_all(root).ok()?;
    let id = key(source);
    let previous = read(root, source, now).path;
    let path = if let Some(png) = png {
        // A new name also invalidates native image caches when a site changes its icon.
        let hash = png.iter().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
        let path = root.join(format!("{id:x}-{hash:x}.png"));
        atomic_write(&path, &png).ok()?;
        Some(path.to_string_lossy().into_owned())
    } else {
        previous
    };
    let entry = Entry {
        source: source.clone(),
        checked: now,
        image: path
            .as_ref()
            .and_then(|p| Path::new(p).file_name()?.to_str().map(str::to_owned)),
    };
    atomic_write(
        &root.join(format!("{id:x}.json")),
        &serde_json::to_vec(&entry).ok()?,
    )
    .ok()?;
    path
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    #[cfg(target_arch = "wasm32")]
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, path)
}

pub fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    let thumb = image.thumbnail(64, 64).to_rgba8();
    let mut png = Cursor::new(Vec::new());
    thumb.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

pub fn root() -> PathBuf {
    daynews_core::cache_dir().join("feed-icons-v1")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thumbnails_and_cache_survive_relaunch_and_expire_without_losing_good_art() {
        let root = std::env::temp_dir().join(format!("day-news-icon-test-{}", std::process::id()));
        let source = Source {
            feed: "https://fixture.example/feed".into(),
            home: None,
            icon: None,
        };
        let image = image::RgbaImage::from_pixel(128, 128, image::Rgba([200, 30, 20, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let png = thumbnail(bytes.get_ref()).unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().width(), 64);
        let path = write(&root, &source, Some(png), 100).unwrap();
        assert!(read(&root, &source, 101).fresh);
        assert!(!read(&root, &source, 100 + 8 * 86400).fresh);
        assert_eq!(write(&root, &source, None, 200), Some(path.clone()));
        std::fs::remove_file(path).unwrap();
        assert!(!read(&root, &source, 201).fresh);
        write(&root, &source, None, 300);
        assert!(read(&root, &source, 301).fresh);
        assert!(!read(&root, &source, 300 + 86400).fresh);
        assert!(thumbnail(b"<html>not an icon</html>").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
