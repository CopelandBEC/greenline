//! Markups proposed by agents, waiting for review in the app.
//!
//! `greenline-cli apply` adds a batch to `<markups file>.proposals.json`,
//! never to the markups file itself (the app holds that one locked while the
//! PDF is open). The app shows the batches and moves what the reviewer accepts
//! into the markups. Both sides read and rewrite the file under its own lock.

use crate::annot::Annot;
use crate::measure::Scale;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// Groups of this batch already accepted, to the group they became, so a
    /// markup accepted later joins the rest of its group.
    #[serde(default)]
    pub accepted_groups: HashMap<u64, u64>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Proposals {
    #[serde(default)]
    pub pdf: PathBuf,
    #[serde(default)]
    pub batches: Vec<Proposal>,
    /// The last batch id given out. Ids are never reused, so a click on a
    /// batch the reviewer saw can't land on a different one.
    #[serde(default)]
    pub seq: u64,
}

impl Proposals {
    /// A new batch id.
    pub fn take_id(&mut self) -> String {
        self.seq = self.seq.max(self.batches.iter().filter_map(|b| b.id.parse::<u64>().ok()).max().unwrap_or(0)) + 1;
        self.seq.to_string()
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
    load_file(&file_for(pdf))
}

pub fn load_file(f: &Path) -> anyhow::Result<Proposals> {
    if !f.exists() {
        return Ok(Proposals::default());
    }
    let p: Proposals = serde_json::from_slice(&std::fs::read(f)?).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
    Ok(p)
}

/// When a proposals file last changed, to notice a new batch.
pub fn modified(f: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(f).and_then(|m| m.modified()).ok()
}

/// Reads, changes and writes the proposals under their lock. The file stays
/// when the last batch goes: it keeps the id counter.
pub fn update<T>(pdf: &Path, f: impl FnOnce(&mut Proposals) -> anyhow::Result<T>) -> anyhow::Result<T> {
    update_file(&file_for(pdf), pdf, f)
}

/// `update` on the proposals file `path` for `pdf`.
pub fn update_file<T>(path: &Path, pdf: &Path, f: impl FnOnce(&mut Proposals) -> anyhow::Result<T>) -> anyhow::Result<T> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut p = load_file(path)?;
    p.pdf = pdf.to_path_buf();
    let out = f(&mut p)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&p)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(out)
}
