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

/// Bold, italic and bold-italic faces of the first (Helvetica-metric) face, for
/// formatted markup text on screen; `None` where the system lacks one.
#[cfg(not(windows))]
fn styled_candidates(bold: bool, italic: bool) -> Vec<String> {
    let name = match (bold, italic) {
        (true, true) => "LiberationSans-BoldItalic.ttf",
        (true, false) => "LiberationSans-Bold.ttf",
        (false, true) => "LiberationSans-Italic.ttf",
        (false, false) => "LiberationSans-Regular.ttf",
    };
    ["/usr/share/fonts/liberation", "/usr/share/fonts/TTF", "/usr/share/fonts/truetype/liberation"].iter().map(|d| format!("{d}/{name}")).collect()
}

#[cfg(windows)]
fn styled_candidates(bold: bool, italic: bool) -> Vec<String> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let name = match (bold, italic) {
        (true, true) => "arialbi.ttf",
        (true, false) => "arialbd.ttf",
        (false, true) => "ariali.ttf",
        (false, false) => "arial.ttf",
    };
    vec![format!(r"{dir}\Fonts\{name}")]
}

/// The styled face's font data, if the system has it.
pub fn styled(bold: bool, italic: bool) -> Option<&'static [u8]> {
    static STYLED: OnceLock<[Option<Vec<u8>>; 3]> = OnceLock::new();
    let all = STYLED.get_or_init(|| {
        let load = |b, i| styled_candidates(b, i).into_iter().find_map(|p| std::fs::read(p).ok());
        [load(true, false), load(false, true), load(true, true)]
    });
    match (bold, italic) {
        (true, false) => all[0].as_deref(),
        (false, true) => all[1].as_deref(),
        (true, true) => all[2].as_deref(),
        (false, false) => None,
    }
}

/// Index into `faces()` of the first face with a glyph for `c`.
pub fn face_for(c: char) -> Option<usize> {
    faces().iter().position(|f| f.covers(c))
}

/// Advance width (1/1000 em) from the first face covering `c`.
pub fn advance(c: char) -> Option<f32> {
    faces().iter().find_map(|f| f.advance(c))
}
