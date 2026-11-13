//! Markups proposed by agents, waiting for review in the app.
//!
//! `greenline-cli apply` adds a batch to `<markups file>.proposals.json`,
//! never to the markups file itself (the app holds that one locked while the
//! PDF is open). The app shows the batches and moves what the reviewer accepts
//! into the markups. Both sides read and rewrite the file under its own lock.

use crate::annot::Annot;
use crate::measure::Scale;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Proposal {
    /// Short id, unique among the file's batches ("1", "2", …).
    pub id: String,
    /// Who proposed it: the agent's name.
    pub agent: String,
    /// One line on what it is ("Window takeoff, sheets A2.0-A2.3").
    #[serde(default)]
    pub summary: String,
    /// Local time, RFC 3339.
    pub created: String,
    #[serde(default)]
    pub annots: Vec<Annot>,
    #[serde(default)]
    pub scales: Vec<Scale>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Proposals {
    #[serde(default)]
    pub pdf: PathBuf,
    #[serde(default)]
    pub batches: Vec<Proposal>,
}

impl Proposals {
    pub fn next_id(&self) -> String {
        (self.batches.iter().filter_map(|b| b.id.parse::<u64>().ok()).max().unwrap_or(0) + 1).to_string()
    }

    /// First free markup id and group id across all batches.
    pub fn next_ids(&self) -> (u64, u64) {
        let annots = self.batches.iter().flat_map(|b| &b.annots);
        let id = annots.clone().map(|a| a.id + 1).max().unwrap_or(1);
        let group = annots.filter_map(|a| a.group).max().unwrap_or(0) + 1;
        (id, group)
    }

    pub fn count(&self) -> usize {
        self.batches.iter().map(|b| b.annots.len() + b.scales.len()).sum()
    }
}

pub fn file_for(pdf: &Path) -> PathBuf {
    crate::sidecar::file_for(pdf).with_extension("proposals.json")
}

/// Reads the proposals for `pdf` (none if there's no file).
pub fn load(pdf: &Path) -> anyhow::Result<Proposals> {
    let f = file_for(pdf);
    if !f.exists() {
        return Ok(Proposals { pdf: pdf.to_path_buf(), batches: Vec::new() });
    }
    let p: Proposals = serde_json::from_slice(&std::fs::read(&f)?).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
    Ok(p)
}

/// When the proposals file last changed, to notice a new batch.
pub fn modified(pdf: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(file_for(pdf)).and_then(|m| m.modified()).ok()
}

/// Reads, changes and writes the proposals under their lock. The file is
/// removed when the last batch goes.
pub fn update<T>(pdf: &Path, f: impl FnOnce(&mut Proposals) -> anyhow::Result<T>) -> anyhow::Result<T> {
    std::fs::create_dir_all(crate::sidecar::dir())?;
    let path = file_for(pdf);
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut p = load(pdf)?;
    p.pdf = pdf.to_path_buf();
    let out = f(&mut p)?;
    if p.batches.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
    } else {
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&p)?)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(out)
}
