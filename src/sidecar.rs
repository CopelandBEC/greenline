//! Autosaved markups, kept outside the PDF's folder (often a shared drive) in
//! ~/.local/share/greenline/<hash of the PDF path>.json.

use crate::annot::Annot;
use crate::measure::{Scale, Units};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Serialize, Deserialize)]
pub struct Sidecar {
    pub pdf: PathBuf,
    pub annots: Vec<Annot>,
    /// Drawing scales set on its pages (files from before 1.5 have none).
    #[serde(default)]
    pub scales: Vec<Scale>,
}

/// ~/.local/share/greenline on Linux, %APPDATA%\\Greenline on Windows.
/// The first call moves the folder over from the app's old name (Redline);
/// if that fails (e.g. an old copy still has it open) the old folder is used.
pub fn dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        #[cfg(windows)]
        let (base, name, old) = (PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()), "Greenline", "Redline");
        #[cfg(not(windows))]
        let (base, name, old) = (
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")),
            "greenline",
            "redline",
        );
        let (dir, old) = (base.join(name), base.join(old));
        if !dir.exists() && old.is_dir() && std::fs::rename(&old, &dir).is_err() {
            return old;
        }
        dir
    })
    .clone()
}

/// FNV-1a: stable across Rust versions, unlike DefaultHasher.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn file_for(pdf: &Path) -> PathBuf {
    let h = fnv(pdf.as_os_str().as_encoded_bytes());
    let stem = pdf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    dir().join(format!("{stem}-{h:016x}.json"))
}

pub fn load(pdf: &Path) -> anyhow::Result<Option<Sidecar>> {
    let f = file_for(pdf);
    if !f.exists() {
        return Ok(None);
    }
    load_file(&f).map(Some)
}

pub fn load_file(f: &Path) -> anyhow::Result<Sidecar> {
    Ok(serde_json::from_slice(&std::fs::read(f)?)?)
}

/// Window preferences that persist across launches.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Prefs {
    pub show_sidebar: bool,
    pub show_markups: bool,
    pub show_stamps: bool,
    /// Name put on editable-export comments; empty = the account name.
    pub author: String,
    /// chrono format for the stamp panel's date.
    pub date_format: String,
    /// Light, dark, or follow the system.
    pub theme: egui::ThemePreference,
    /// How measurements read: feet-inches or metric.
    pub units: Units,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            show_sidebar: true,
            show_markups: false,
            show_stamps: false,
            author: String::new(),
            date_format: "%m/%d/%Y".into(),
            theme: egui::ThemePreference::System,
            units: Units::Imperial,
        }
    }
}

pub fn load_prefs() -> Prefs {
    std::fs::read(dir().join("prefs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_prefs(p: &Prefs) {
    let _ = std::fs::create_dir_all(dir());
    if let Ok(b) = serde_json::to_vec(p) {
        let _ = std::fs::write(dir().join("prefs.json"), b);
    }
}

/// Exclusive per-document lock so two windows can't overwrite each other's
/// markups. `Ok(None)` means another Greenline process holds it. Released when
/// the returned file is dropped or the process exits.
pub fn lock(pdf: &Path) -> anyhow::Result<Option<std::fs::File>> {
    std::fs::create_dir_all(dir())?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(file_for(pdf).with_extension("lock"))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

pub fn save(pdf: &Path, annots: &[Annot], scales: &[Scale]) -> anyhow::Result<()> {
    save_file(&file_for(pdf), pdf, annots, scales)
}

/// `save` to the markups file `f` for `pdf`.
pub fn save_file(f: &Path, pdf: &Path, annots: &[Annot], scales: &[Scale]) -> anyhow::Result<()> {
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    let f = f.to_path_buf();
    let tmp = f.with_extension(format!("json.{}.tmp", std::process::id()));
    let s = Sidecar { pdf: pdf.to_path_buf(), annots: annots.to_vec(), scales: scales.to_vec() };
    std::fs::write(&tmp, serde_json::to_vec(&s)?)?;
    std::fs::rename(tmp, f)?;
    Ok(())
}
