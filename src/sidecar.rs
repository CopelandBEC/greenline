//! Autosaved markups, kept outside the PDF's folder (often a shared drive) in
//! ~/.local/share/greenline/<hash of the PDF path>.json.

use crate::annot::Annot;
use crate::measure::{Scale, Units};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};

#[derive(Serialize, Deserialize)]
pub struct Sidecar {
    pub pdf: PathBuf,
    pub annots: Vec<Annot>,
    /// Drawing scales set on its pages (files from before 1.5 have none).
    #[serde(default)]
    pub scales: Vec<Scale>,
    /// Proposed markups already taken in (`<batch>/<proposed id>:<markup id>`),
    /// so an accept retried after a half-finished one can't add them twice
    /// and the rest of their groups still join them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<String>,
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
    /// Shared folder of Bluebeam line type (.blx) and hatch (.bhx) files, e.g. a firm's standards.
    pub pattern_dir: Option<PathBuf>,
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
            pattern_dir: None,
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

/// Writes the markups file `f` for `pdf`.
pub fn save_file(f: &Path, pdf: &Path, annots: &[Annot], scales: &[Scale], accepted: &[String]) -> anyhow::Result<()> {
    let s = Sidecar { pdf: pdf.to_path_buf(), annots: annots.to_vec(), scales: scales.to_vec(), accepted: accepted.to_vec() };
    write(f, &s)
}

fn write(f: &Path, s: &Sidecar) -> anyhow::Result<()> {
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    // Unique per write, so two can never share a temporary file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = f.with_extension(format!("json.{}-{seq}.tmp", std::process::id()));
    let written = serde_json::to_vec(s).map_err(anyhow::Error::from).and_then(|b| Ok(std::fs::write(&tmp, b)?)).and_then(|_| Ok(std::fs::rename(&tmp, f)?));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// What the saver is asked to do.
enum Job {
    /// Write `file`; the result goes to `Saver::done` with `seq`.
    Later { file: PathBuf, sidecar: Sidecar, seq: u64 },
    /// Write `file` and answer straight away.
    Now { file: PathBuf, sidecar: Sidecar, reply: mpsc::Sender<Result<(), String>> },
    /// Answer once everything asked before is written.
    Flush(mpsc::Sender<()>),
}

/// Writes markups files on a thread of its own, in the order asked, so
/// autosaving never holds up the window (serialising and writing a big set's
/// markups, or a virus scanner looking at the file, can take a while).
pub struct Saver {
    tx: mpsc::Sender<Job>,
    /// Background saves done: the file, the number they were asked with, and how it went.
    pub done: mpsc::Receiver<(PathBuf, u64, Result<(), String>)>,
}

impl Saver {
    /// `wake` is called when a background save finishes (to repaint the window).
    pub fn spawn(wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut jobs: Vec<Job> = vec![first];
                    jobs.extend(rx.try_iter());
                    for (i, job) in jobs.iter().enumerate() {
                        match job {
                            Job::Later { file, sidecar, seq } => {
                                // A newer write of the same file already waiting makes this one moot.
                                let superseded = jobs[i + 1..].iter().any(|j| matches!(j, Job::Later { file: f, .. } | Job::Now { file: f, .. } if f == file));
                                let result = if superseded { Ok(()) } else { write(file, sidecar).map_err(|e| e.to_string()) };
                                if done_tx.send((file.clone(), *seq, result)).is_err() {
                                    return;
                                }
                                wake();
                            }
                            Job::Now { file, sidecar, reply } => {
                                let _ = reply.send(write(file, sidecar).map_err(|e| e.to_string()));
                            }
                            Job::Flush(reply) => {
                                let _ = reply.send(());
                            }
                        }
                    }
                }
            })
            .expect("autosave thread");
        Self { tx, done }
    }

    /// Writes in the background; the outcome arrives on `done` with `seq`.
    pub fn save_later(&self, file: PathBuf, sidecar: Sidecar, seq: u64) {
        let _ = self.tx.send(Job::Later { file, sidecar, seq });
    }

    /// Writes after any background saves still waiting, and returns how it went.
    pub fn save_now(&self, file: PathBuf, sidecar: Sidecar) -> anyhow::Result<()> {
        let (reply, rx) = mpsc::channel();
        self.tx.send(Job::Now { file, sidecar, reply }).map_err(|_| anyhow::anyhow!("autosave stopped"))?;
        rx.recv().map_err(|_| anyhow::anyhow!("autosave stopped"))?.map_err(anyhow::Error::msg)
    }

    /// Waits until every save asked for so far is written (before writing a file some other way).
    pub fn flush(&self) {
        let (reply, rx) = mpsc::channel();
        if self.tx.send(Job::Flush(reply)).is_ok() {
            let _ = rx.recv();
        }
    }
}
