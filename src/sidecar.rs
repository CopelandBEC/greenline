//! Autosaved markups, kept outside the PDF's folder (often a shared drive) in
//! ~/.local/share/redline/<hash of the PDF path>.json.

use crate::annot::Annot;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
pub struct Sidecar {
    pub pdf: PathBuf,
    pub annots: Vec<Annot>,
}

fn dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    base.join("redline")
}

fn file_for(pdf: &Path) -> PathBuf {
    // FNV-1a: stable across Rust versions, unlike DefaultHasher.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in pdf.as_os_str().as_encoded_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let stem = pdf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    dir().join(format!("{stem}-{h:016x}.json"))
}

pub fn load(pdf: &Path) -> anyhow::Result<Option<Sidecar>> {
    let f = file_for(pdf);
    if !f.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&std::fs::read(f)?)?))
}

/// Window preferences that persist across launches.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub struct Prefs {
    pub show_sidebar: bool,
    pub show_markups: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { show_sidebar: true, show_markups: false }
    }
}

pub fn load_prefs() -> Prefs {
    std::fs::read(dir().join("prefs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_prefs(p: Prefs) {
    let _ = std::fs::create_dir_all(dir());
    if let Ok(b) = serde_json::to_vec(&p) {
        let _ = std::fs::write(dir().join("prefs.json"), b);
    }
}

/// Exclusive per-document lock so two windows can't overwrite each other's
/// markups. `Ok(None)` means another Redline process holds it. Released when
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

pub fn save(pdf: &Path, annots: &[Annot]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir())?;
    let f = file_for(pdf);
    let tmp = f.with_extension(format!("json.{}.tmp", std::process::id()));
    let s = Sidecar { pdf: pdf.to_path_buf(), annots: annots.to_vec() };
    std::fs::write(&tmp, serde_json::to_vec(&s)?)?;
    std::fs::rename(tmp, f)?;
    Ok(())
}
