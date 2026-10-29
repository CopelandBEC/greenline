//! Stamp and signature image library. Imported images are copied (cleaned up
//! where needed) into `<data dir>/images/<hash>.<ext>`, and markups refer to
//! that file name, so removing an entry from the library never breaks a stamp
//! already placed on a sheet.

use egui::{vec2, Rect, Vec2};
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Longest side kept on import: ~1200 dpi on a 2" seal.
const MAX_PX: u32 = 2400;
/// Scanned paper at or above this level counts as white.
const PAPER: f32 = 235.0;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Use {
    Stamp,
    Signature,
}

impl Use {
    /// Placed width (pt) before the user has picked one.
    fn default_width(self) -> f32 {
        match self {
            Use::Stamp => 126.0,
            Use::Signature => 144.0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Entry {
    /// Stored file name (see `path`).
    pub image: String,
    pub name: String,
    pub kind: Use,
    /// Width (pt) it is placed at: the size last drawn or resized to.
    pub width: f32,
    /// Width / height.
    pub aspect: f32,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    pub entries: Vec<Entry>,
}

fn images_dir() -> PathBuf {
    crate::sidecar::dir().join("images")
}

fn library_file() -> PathBuf {
    crate::sidecar::dir().join("stamps.json")
}

/// Path of a stored image. `None` for names that aren't ours: sidecars are
/// plain JSON, so never let one point outside the images folder.
pub fn path(image: &str) -> Option<PathBuf> {
    let ok = image
        .split_once('.')
        .is_some_and(|(h, ext)| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()) && matches!(ext, "png" | "jpg"));
    ok.then(|| images_dir().join(image))
}

pub fn decode(image: &str) -> anyhow::Result<RgbaImage> {
    let p = path(image).ok_or_else(|| anyhow::anyhow!("bad image name {image:?}"))?;
    Ok(image::open(&p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?.to_rgba8())
}

impl Library {
    pub fn load() -> Self {
        std::fs::read(library_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(crate::sidecar::dir())?;
        std::fs::write(library_file(), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Copies `src` into the image store and lists it. Returns the entry's
    /// index and whether a white background was made transparent.
    pub fn import(&mut self, src: &Path, kind: Use) -> anyhow::Result<(usize, bool)> {
        let p = prepare(&std::fs::read(src)?)?;
        let image = format!("{:016x}.{}", crate::sidecar::fnv(&p.bytes), p.ext);
        let dest = images_dir().join(&image);
        if !dest.exists() {
            std::fs::create_dir_all(images_dir())?;
            let tmp = dest.with_extension(format!("{}.{}.tmp", p.ext, std::process::id()));
            std::fs::write(&tmp, &p.bytes)?;
            std::fs::rename(tmp, &dest)?;
        }
        if let Some(i) = self.entries.iter().position(|e| e.image == image && e.kind == kind) {
            return Ok((i, p.knocked_out));
        }
        let name = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Stamp".into());
        let aspect = p.w as f32 / p.h.max(1) as f32;
        self.entries.push(Entry { image, name, kind, width: kind.default_width(), aspect });
        self.save()?;
        Ok((self.entries.len() - 1, p.knocked_out))
    }
}

struct Prepared {
    bytes: Vec<u8>,
    ext: &'static str,
    w: u32,
    h: u32,
    knocked_out: bool,
}

/// Decodes an imported file and caps its size. A scan with no transparency gets
/// its paper knocked out so title-block lines show through, like a wet stamp.
/// Untouched files are kept byte for byte; changed ones are re-encoded as PNG.
fn prepare(bytes: &[u8]) -> anyhow::Result<Prepared> {
    let format = image::guess_format(bytes)?;
    let ext = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        _ => anyhow::bail!("only PNG and JPEG images are supported"),
    };
    let mut img = image::load_from_memory_with_format(bytes, format)?.to_rgba8();
    let mut changed = false;
    if img.width().max(img.height()) > MAX_PX {
        let k = MAX_PX as f32 / img.width().max(img.height()) as f32;
        let (w, h) = (((img.width() as f32 * k).round() as u32).max(1), ((img.height() as f32 * k).round() as u32).max(1));
        img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
        changed = true;
    }
    let opaque = img.pixels().all(|p| p[3] == 255);
    if opaque {
        knock_out_white(&mut img);
        changed = true;
    }
    let (w, h) = img.dimensions();
    if !changed {
        return Ok(Prepared { bytes: bytes.to_vec(), ext, w, h, knocked_out: false });
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(Prepared { bytes: out.into_inner(), ext: "png", w, h, knocked_out: opaque })
}

/// Turns white paper transparent, un-blending anti-aliased edges from white
/// so ink keeps its colour at any opacity.
fn knock_out_white(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let lift = |c: u8| (c as f32 * 255.0 / PAPER).min(255.0);
        let (r, g, b) = (lift(p[0]), lift(p[1]), lift(p[2]));
        let a = 1.0 - r.min(g).min(b) / 255.0;
        if a <= 0.0 {
            *p = Rgba([255, 255, 255, 0]);
            continue;
        }
        let un = |c: f32| ((c - 255.0 * (1.0 - a)) / a).clamp(0.0, 255.0).round() as u8;
        *p = Rgba([un(r), un(g), un(b), (a * 255.0).round() as u8]);
    }
}

// ---------------------------------------------------------------------------
// Dates and page sets

/// Date formats offered in the stamp panel (chrono syntax).
pub const DATE_FORMATS: [&str; 4] = ["%m/%d/%Y", "%Y-%m-%d", "%b %-d, %Y", "%-d %B %Y"];

/// Today's local date. Unknown formats (prefs are hand-editable) fall back to
/// the first, since chrono panics on a bad format string.
pub fn today(fmt: &str) -> String {
    let fmt = if DATE_FORMATS.contains(&fmt) { fmt } else { DATE_FORMATS[0] };
    chrono::Local::now().format(fmt).to_string()
}

/// Parses "2-5, 8, 10-" (1-based, inclusive; open end = last page) into
/// 0-based page indices in order, without duplicates.
pub fn parse_pages(s: &str, n: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let num = |t: &str, default: usize| -> Result<usize, String> {
            let t = t.trim();
            if t.is_empty() {
                return Ok(default);
            }
            t.parse::<usize>().ok().filter(|&v| (1..=n).contains(&v)).ok_or_else(|| format!("\"{t}\" isn't a page (1–{n})"))
        };
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (num(a, 1)?, num(b, n)?),
            None => {
                let v = num(part, 1)?;
                (v, v)
            }
        };
        if a > b {
            return Err(format!("\"{part}\" runs backwards"));
        }
        for p in a - 1..b {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    if out.is_empty() {
        return Err("no pages given".into());
    }
    Ok(out)
}

/// Offset that keeps markups `b` on a page of size `from` the same distance
/// from that page's nearest corner on a page of size `to`. Title blocks sit at
/// a corner, so a stamp stays in its box across sheets of different sizes.
pub fn corner_offset(b: Rect, from: Vec2, to: Vec2) -> Vec2 {
    let c = b.center();
    vec2(if c.x > from.x / 2.0 { to.x - from.x } else { 0.0 }, if c.y > from.y / 2.0 { to.y - from.y } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn knock_out_keeps_ink_and_clears_paper() {
        let mut img = RgbaImage::from_pixel(4, 1, Rgba([240, 240, 240, 255]));
        img.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        img.put_pixel(2, 0, Rgba([0, 40, 200, 255]));
        img.put_pixel(3, 0, Rgba([128, 128, 128, 255]));
        knock_out_white(&mut img);
        assert_eq!(img.get_pixel(0, 0)[3], 0, "paper");
        assert_eq!(*img.get_pixel(1, 0), Rgba([0, 0, 0, 255]), "black ink");
        assert_eq!(*img.get_pixel(2, 0), Rgba([0, 43, 217, 255]), "blue ink keeps its hue");
        let edge = img.get_pixel(3, 0);
        assert!(edge[3] > 100 && edge[3] < 140 && edge[0] < 5, "anti-aliased edge becomes part-transparent black: {edge:?}");
    }

    #[test]
    fn prepare_keeps_transparent_png_and_cleans_opaque_jpeg() {
        let mut png = RgbaImage::from_pixel(10, 5, Rgba([0, 0, 0, 0]));
        png.put_pixel(2, 2, Rgba([0, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let bytes = bytes.into_inner();
        let p = prepare(&bytes).unwrap();
        assert_eq!((p.ext, p.w, p.h, p.knocked_out), ("png", 10, 5, false));
        assert_eq!(p.bytes, bytes, "kept byte for byte");

        let jpg = image::RgbImage::from_pixel(3000, 1000, image::Rgb([250, 250, 250]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        jpg.write_to(&mut bytes, image::ImageFormat::Jpeg).unwrap();
        let p = prepare(&bytes.into_inner()).unwrap();
        assert_eq!((p.ext, p.w, p.h, p.knocked_out), ("png", 2400, 800, true));
        assert!(prepare(b"GIF89a....").is_err());
    }

    #[test]
    fn stored_names_stay_in_the_store() {
        assert!(path("0123abcd.png").is_some());
        for bad in ["../x.png", "abc.exe", "/etc/passwd", "ab/cd.png", ".png", "x"] {
            assert!(path(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn page_sets() {
        assert_eq!(parse_pages("2-4, 7", 10), Ok(vec![1, 2, 3, 6]));
        assert_eq!(parse_pages("8-", 10), Ok(vec![7, 8, 9]));
        assert_eq!(parse_pages("3,3,2-3", 10), Ok(vec![2, 1]));
        assert!(parse_pages("0", 10).is_err());
        assert!(parse_pages("11", 10).is_err());
        assert!(parse_pages("5-2", 10).is_err());
        assert!(parse_pages(" , ", 10).is_err());
    }

    #[test]
    fn corner_offset_follows_nearest_corner() {
        let title_block = Rect::from_min_size(pos2(2900.0, 2000.0), vec2(150.0, 150.0));
        let (d, e) = (vec2(3024.0, 2160.0), vec2(2592.0, 1728.0));
        assert_eq!(corner_offset(title_block, d, e), vec2(-432.0, -432.0), "bottom-right stays bottom-right");
        let top_left = Rect::from_min_size(pos2(50.0, 50.0), vec2(100.0, 100.0));
        assert_eq!(corner_offset(top_left, d, e), Vec2::ZERO);
        assert_eq!(corner_offset(title_block, d, d), Vec2::ZERO, "same size: same place");
    }

    #[test]
    fn today_never_panics_on_odd_formats() {
        assert!(!today("%Q nonsense").is_empty());
        for f in DATE_FORMATS {
            assert!(today(f).contains(&chrono::Local::now().format("%Y").to_string()));
        }
    }
}
