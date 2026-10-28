//! System fonts used for characters outside Helvetica's WinAnsi set: glyph
//! coverage and advance widths for layout, and file paths for embedding.

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};
use std::sync::OnceLock;

/// In preference order. Liberation Sans is metric-compatible with Helvetica.
const CANDIDATES: [&str; 6] = [
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/TTF/LiberationSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansMath-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
];

pub struct Face {
    pub path: &'static str,
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
        CANDIDATES.iter().filter_map(|p| std::fs::read(p).ok().map(|data| Face { path: p, data })).collect()
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
