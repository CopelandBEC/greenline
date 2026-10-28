//! System fonts used for characters outside Helvetica's WinAnsi set: glyph
//! coverage and advance widths for layout, and file paths for embedding.

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};
use std::sync::OnceLock;

/// In preference order. The first face is metric-compatible with Helvetica
/// (Liberation Sans on Linux, Arial on Windows), so screen layout matches export.
#[cfg(not(windows))]
fn candidates() -> Vec<String> {
    [
        "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/LiberationSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansMath-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
        // Debian/Ubuntu layout
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMath-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
    ]
    .map(String::from)
    .to_vec()
}

#[cfg(windows)]
fn candidates() -> Vec<String> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    ["arial.ttf", "segoeui.ttf", "seguisym.ttf"].iter().map(|f| format!(r"{dir}\Fonts\{f}")).collect()
}

pub struct Face {
    pub path: String,
    data: Vec<u8>,
}

impl Face {
    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::new(&self.data).ok()
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn covers(&self, c: char) -> bool {
        self.font().is_some_and(|f| f.charmap().map(c).is_some())
    }

    /// Advance width in 1/1000 em.
    pub fn advance(&self, c: char) -> Option<f32> {
        let f = self.font()?;
        let gid = f.charmap().map(c)?;
        f.glyph_metrics(Size::new(1000.0), LocationRef::default()).advance_width(gid)
    }
}

pub fn faces() -> &'static [Face] {
    static FACES: OnceLock<Vec<Face>> = OnceLock::new();
    FACES.get_or_init(|| {
        candidates().into_iter().filter_map(|path| std::fs::read(&path).ok().map(|data| Face { path, data })).collect()
    })
}

/// Index into `faces()` of the first face with a glyph for `c`.
pub fn face_for(c: char) -> Option<usize> {
    faces().iter().position(|f| f.covers(c))
}

/// Advance width (1/1000 em) from the first face covering `c`.
pub fn advance(c: char) -> Option<f32> {
    faces().iter().find_map(|f| f.advance(c))
}
