//! The std directory on disk as a [`StdTree`] (#1115), for the language server: `std.…` completion lists its
//! folders and files, and stamps its index by the files' lengths and modification times. The editor's unsaved
//! texts (by canonical path) come before the files. A wasm32 build has no file system and does not compile this
//! module; the playground bundles its std instead (okkama-wasm).

use super::std_paths::{Stamp, StdTree, is_segment};
use crate::source::SourceFile;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Unsaved editor texts by canonical path.
pub type Overlay = HashMap<PathBuf, Rc<str>>;

/// The std directory `root`, with the editor's unsaved texts.
#[derive(Clone, Copy, Debug)]
pub struct DiskStd<'a> {
    root: &'a Path,
    overlay: Option<&'a Overlay>,
}

impl<'a> DiskStd<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self { root, overlay: None }
    }

    pub fn with_overlay(root: &'a Path, overlay: &'a Overlay) -> Self {
        Self { root, overlay: Some(overlay) }
    }

    fn file(&self, module: &str) -> PathBuf {
        self.root.join(format!("{module}.ok"))
    }
}

impl StdTree for DiskStd<'_> {
    fn key(&self) -> String {
        self.root.display().to_string()
    }

    fn modules(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect(self.root, self.root, &mut out);
        out
    }

    fn stamp(&self, modules: &[String]) -> Option<Stamp> {
        let stamps = modules.iter().map(|module| {
            let metadata = std::fs::metadata(self.file(module)).ok()?;
            let modified = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            Some((module.clone(), metadata.len(), modified))
        });
        stamps.collect::<Option<Vec<_>>>().map(Stamp)
    }

    fn entries(&self, folder: &[String]) -> Vec<(String, bool)> {
        let directory = folder.iter().fold(self.root.to_path_buf(), |dir, segment| dir.join(segment));
        let Ok(entries) = std::fs::read_dir(&directory) else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.file_type().ok()?.is_dir() {
                    Some((name, true))
                } else {
                    name.strip_suffix(".ok").map(|stem| (stem.to_string(), false))
                }
            })
            .collect()
    }

    fn source(&self, module: &str) -> Option<SourceFile> {
        let file = self.file(module);
        let text = std::fs::read_to_string(&file).ok()?;
        SourceFile::new(file.display().to_string(), text).ok()
    }

    fn unsaved(&self, module: &str) -> Option<SourceFile> {
        let overlay = self.overlay.filter(|overlay| !overlay.is_empty())?;
        let file = self.file(module);
        let text = overlay.get(&file.canonicalize().unwrap_or_else(|_| file.clone()))?;
        SourceFile::new(file.display().to_string(), text.to_string()).ok()
    }
}

/// Пути модулей (`io/stdout`) каталога `dir` и вложенных.
fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if is_segment(&name) {
                collect(root, &path, out);
            }
        } else if let Some(stem) = name.strip_suffix(".ok")
            && is_segment(stem)
            && let Ok(relative) = path.with_extension("").strip_prefix(root)
        {
            let parts: Vec<String> =
                relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
            out.push(parts.join("/"));
        }
    }
}
