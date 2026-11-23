//! Дополнение путей `std.…` в редакторе (`okkama lsp`, §5.2.1).
//!
//! Путь у курсора читается назад от него: `std.fs.pa|`. Пока сегменты
//! строчные, это каталоги и модули std: предлагаются папки и файлы
//! текущего уровня, а если путь уже назвал модуль (`std.io.stdout.`) — его
//! объявления. Целые пути (`io.stdout`, а с первой набранной буквой и
//! `io.stdout.Stdout`) ищутся по всей std. После enum (`std.compare.Order.`)
//! предлагаются его варианты. Каталог читается при каждом запросе, разбор
//! модулей для целых путей кэшируется на время работы сервера.
use crate::source::SourceFile;
use crate::syntax::{self, ParseMode, ParseOptions, ast};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::rc::Rc;

/// Вид предложения пути std.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdKind {
    Folder,
    Module,
    Object,
    Role,
    Enum,
    EnumCase,
    Native,
}

/// Предложение дополнения пути std.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdCompletion {
    /// Вставляемый текст: сегмент (`stdout`) или целый путь (`io.stdout.Stdout`).
    pub label: String,
    pub kind: StdKind,
    pub detail: String,
    pub documentation: String,
    /// Целый путь из глубины std, а не элемент текущего уровня.
    pub deep: bool,
    /// Module and optional declaration for lazy LSP documentation lookup.
    pub documentation_source: Option<(String, Option<String>)>,
}

/// Предложения для курсора `offset`; `None` — курсор не в пути `std.…`.
pub fn std_completions(text: &str, offset: u32, stdlib: &Path) -> Option<Vec<StdCompletion>> {
    std_completions_with_overlay(text, offset, stdlib, &HashMap::new())
}

/// Completion inventory with the editor's current source texts. Unchanged
/// files share the disk index; an overlay replaces the whole public inventory.
pub fn std_completions_with_overlay(
    text: &str, offset: u32, stdlib: &Path, overlay: &HashMap<PathBuf, Rc<str>>,
) -> Option<Vec<StdCompletion>> {
    let (segments, partial) = path_at(text, offset as usize)?;
    if let Some(last) = segments.last().filter(|segment| segment.starts_with(char::is_uppercase)) {
        // `std.module.Enum.`: варианты enum.
        let module = segments[..segments.len() - 1].join("/");
        let declarations = declarations(&stdlib.join(format!("{module}.ok")), overlay);
        let cases = declarations
            .into_iter()
            .find(|decl| decl.name == *last)
            .map(|decl| decl.cases)
            .unwrap_or_default();
        return Some(
            cases
                .into_iter()
                .map(|case| StdCompletion {
                    detail: format!("{last}.{case}"),
                    label: case,
                    kind: StdKind::EnumCase,
                    documentation: String::new(),
                    deep: false,
                    documentation_source: Some((module.clone(), Some(last.clone()))),
                })
                .collect(),
        );
    }
    let directory = segments.iter().fold(stdlib.to_path_buf(), |dir, segment| dir.join(segment));
    let mut items = Vec::new();
    // Папки и модули текущего уровня.
    if let Ok(entries) = std::fs::read_dir(&directory) {
        let mut entries: Vec<(String, bool)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let folder = entry.file_type().ok()?.is_dir();
                if folder {
                    is_segment(&name).then_some((name, true))
                } else {
                    let stem = name.strip_suffix(".ok")?;
                    is_segment(stem).then(|| (stem.to_string(), false))
                }
            })
            .collect();
        entries.sort();
        entries.dedup();
        for (name, folder) in entries {
            let path = segments.iter().cloned().chain([name.clone()]).collect::<Vec<_>>();
            if folder {
                items.push(StdCompletion {
                    detail: format!("std.{} (folder)", path.join(".")),
                    label: name,
                    kind: StdKind::Folder,
                    documentation: String::new(),
                    deep: false,
                    documentation_source: None,
                });
            } else {
                let module = path.join("/");
                items.push(StdCompletion {
                    detail: format!("module {module}"),
                    documentation: module_doc(&stdlib.join(format!("{module}.ok")), overlay),
                    label: name,
                    kind: StdKind::Module,
                    documentation_source: Some((module.clone(), None)),
                    deep: false,
                });
            }
        }
    }
    // Путь уже назвал модуль: его объявления.
    if !segments.is_empty() {
        let module = segments.join("/");
        for decl in declarations(&stdlib.join(format!("{module}.ok")), overlay) {
            items.push(decl.completion(decl.name.clone(), false));
        }
    }
    // Целые пути из глубины: модули, а с набранной буквой — и объявления.
    let prefix = segments.join("/");
    let base = deep_index(stdlib);
    let mut index = Arc::clone(&base);
    if !overlay.is_empty() {
        for (position, module) in base.iter().enumerate() {
            let file = stdlib.join(format!("{}.ok", module.path));
            if overlay.contains_key(&file.canonicalize().unwrap_or_else(|_| file.clone())) {
                let module = &mut Arc::make_mut(&mut index)[position];
                module.doc = module_doc(&file, overlay);
                module.declarations = declarations(&file, overlay);
            }
        }
    }
    let lower = partial.to_lowercase();
    for module in index.iter() {
        let Some(rest) = relative(&module.path, &prefix) else {
            continue;
        };
        if rest.contains('/') {
            items.push(StdCompletion {
                label: rest.replace('/', "."),
                kind: StdKind::Module,
                detail: format!("module {}", module.path),
                documentation: module.doc.clone(),
                deep: true,
                documentation_source: Some((module.path.clone(), None)),
            });
        }
        if !lower.is_empty() && !rest.is_empty() {
            for decl in &module.declarations {
                if decl.name.to_lowercase().contains(&lower) {
                    items.push(decl.completion(format!("{}.{}", rest.replace('/', "."), decl.name), true));
                }
            }
        }
    }
    Some(items)
}

/// Сегменты пути до курсора (без `std`) и набираемый сегмент.
fn path_at(text: &str, offset: usize) -> Option<(Vec<String>, String)> {
    let before = text.get(..offset)?;
    let word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let partial_start = before.rfind(|ch: char| !word(ch)).map_or(0, |index| index + 1);
    let partial = before[partial_start..].to_string();
    let mut segments = Vec::new();
    let mut end = partial_start;
    loop {
        // Перед сегментом — точка, перед ней — следующий сегмент назад.
        if !before[..end].ends_with('.') {
            return None;
        }
        let dot = end - 1;
        let start = before[..dot].rfind(|ch: char| !word(ch)).map_or(0, |index| index + 1);
        let segment = &before[start..dot];
        if segment.is_empty() {
            return None;
        }
        if segment == "std" {
            // `std` — начало пути, а не поле: перед ним не точка и не буква.
            if before[..start].ends_with('.') {
                return None;
            }
            segments.reverse();
            return Some((segments, partial));
        }
        segments.push(segment.to_string());
        end = start;
    }
}

/// Сегмент пути модуля: имя каталога или файла std со строчной буквы.
fn is_segment(name: &str) -> bool {
    name.starts_with(|ch: char| ch.is_ascii_lowercase())
        && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// `module` внутри `prefix` (`io/stdout` в `io` → `stdout`); `None` — снаружи.
fn relative<'a>(module: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(module);
    }
    module.strip_prefix(prefix)?.strip_prefix('/')
}

#[derive(Clone, Debug)]
struct Declaration {
    module: String,
    name: String,
    kind: StdKind,
    detail: String,
    doc: String,
    cases: Vec<String>,
}

impl Declaration {
    fn completion(&self, label: String, deep: bool) -> StdCompletion {
        StdCompletion {
            label,
            kind: self.kind,
            detail: self.detail.clone(),
            documentation: self.doc.clone(),
            deep,
            documentation_source: Some((self.module.clone(), Some(self.name.clone()))),
        }
    }
}

#[derive(Clone, Debug)]
struct Module {
    /// `io/stdout`.
    path: String,
    doc: String,
    declarations: Vec<Declaration>,
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    path: String,
    length: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Debug)]
struct CachedIndex {
    stamps: Vec<FileStamp>,
    modules: Arc<Vec<Module>>,
}

/// Cache disk declarations until the source inventory or file metadata changes.
/// Editor overlays are applied after this cache and never stored in it.
fn deep_index(stdlib: &Path) -> Arc<Vec<Module>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedIndex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut files = Vec::new();
    collect(stdlib, stdlib, &mut files);
    files.sort();
    let stamps: Vec<_> = files.iter().map(|path| {
        let metadata = std::fs::metadata(stdlib.join(format!("{path}.ok"))).ok();
        FileStamp {
            path: path.clone(),
            length: metadata.as_ref().map_or(0, std::fs::Metadata::len),
            modified: metadata.and_then(|metadata| metadata.modified().ok()),
        }
    }).collect();
    if stamps.iter().all(|stamp| stamp.modified.is_some())
        && let Some(found) = cache.lock().ok().and_then(|cache| {
            cache.get(stdlib).filter(|found| found.stamps == stamps).map(|found| Arc::clone(&found.modules))
        })
    {
        return found;
    }
    let modules = Arc::new(files.into_iter().map(|path| {
        // One read and parse per file for both (#1008).
        let file = stdlib.join(format!("{path}.ok"));
        let (doc, declarations) = match file_doc(&file, &HashMap::new()) {
            Some((doc, program)) => {
                let declarations = declarations_of(&doc, &program);
                (doc.overview.map(|overview| overview.markdown()).unwrap_or_default(), declarations)
            }
            None => (String::new(), Vec::new()),
        };
        Module { doc, declarations, path }
    }).collect::<Vec<_>>());
    if let Ok(mut cache) = cache.lock() {
        cache.insert(stdlib.to_path_buf(), CachedIndex { stamps, modules: Arc::clone(&modules) });
    }
    modules
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

/// Public declarations and documentation come from the canonical model.
fn declarations(file: &Path, overlay: &HashMap<PathBuf, Rc<str>>) -> Vec<Declaration> {
    let Some((doc, program)) = file_doc(file, overlay) else { return Vec::new(); };
    declarations_of(&doc, &program)
}

fn declarations_of(doc: &crate::doc::ModuleDoc, program: &ast::Program) -> Vec<Declaration> {
    doc.items.iter().filter_map(|item| {
        let kind = match item.kind {
            crate::doc::ItemKind::Object => StdKind::Object,
            crate::doc::ItemKind::Role => StdKind::Role,
            crate::doc::ItemKind::Enum | crate::doc::ItemKind::Error => StdKind::Enum,
            crate::doc::ItemKind::Native => StdKind::Native,
            _ => return None,
        };
        let cases = program.items.iter().find_map(|decl| match decl {
            ast::Item::Enum(decl) if decl.name.text == item.name => Some(decl.cases.iter().map(|case| case.text.clone()).collect()),
            _ => None,
        }).unwrap_or_default();
        Some(Declaration {
            module: doc.path.clone(), name: item.name.clone(), kind,
            detail: item.signature.clone(), doc: crate::doc::render::hover(doc, item, None), cases,
        })
    }).collect()
}

fn file_doc(file: &Path, overlay: &HashMap<PathBuf, Rc<str>>) -> Option<(crate::doc::ModuleDoc, ast::Program)> {
    let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let text = overlay.get(&canonical).map(|text| text.to_string()).or_else(|| std::fs::read_to_string(file).ok())?;
    let source = SourceFile::new(file.display().to_string(), text).ok()?;
    let options = ParseOptions { mode: ParseMode::Production, ..ParseOptions::default() };
    let program = syntax::parse(&source, &options).ok()?;
    Some((crate::doc::ModuleDoc::of(&source, &program), program))
}

fn module_doc(file: &Path, overlay: &HashMap<PathBuf, Rc<str>>) -> String {
    file_doc(file, overlay).and_then(|(doc, _)| doc.overview).map(|doc| doc.markdown()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{StdKind, std_completions};
    use std::path::PathBuf;

    fn stdlib() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std")
    }

    fn labels(text: &str) -> Option<Vec<(String, StdKind, bool)>> {
        std_completions(text, text.len() as u32, &stdlib())
            .map(|items| items.into_iter().map(|item| (item.label, item.kind, item.deep)).collect())
    }

    #[test]
    fn std_lists_folders_modules_and_whole_paths() {
        let found = labels("let out = std.").unwrap();
        assert!(found.contains(&("io".into(), StdKind::Folder, false)));
        assert!(found.contains(&("compare".into(), StdKind::Module, false)));
        assert!(found.contains(&("io.stdout".into(), StdKind::Module, true)));
        let found = labels("let out = std.io.").unwrap();
        assert!(found.contains(&("stdout".into(), StdKind::Module, false)));
        assert!(!found.iter().any(|(label, ..)| label == "io"));
    }

    #[test]
    fn module_path_lists_declarations_and_enum_cases() {
        let found = labels("std.io.stdout.").unwrap();
        assert!(found.contains(&("Stdout".into(), StdKind::Object, false)));
        let found = labels("std.compare.Order.").unwrap();
        assert!(found.contains(&("less".into(), StdKind::EnumCase, false)));
        let found = labels("std.runtime.debug.").unwrap();
        assert!(found.contains(&("data_dump".into(), StdKind::Native, false)));
        // С набранной буквой — объявления из глубины std.
        let found = labels("std.Stdo").unwrap();
        assert!(found.contains(&("io.stdout.Stdout".into(), StdKind::Object, true)));
    }

    #[test]
    fn other_paths_are_not_std() {
        assert_eq!(labels("let x = out."), None);
        assert_eq!(labels("let x = self.std."), None);
        assert_eq!(labels("let x = mystd."), None);
    }

    #[test]
    fn documentation_uses_public_model_and_unsaved_std_sources() {
        let dir = std::env::temp_dir().join(format!("okkama-std-doc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("demo.ok");
        let text = "module demo;\n\n// Module overview.\n\n// A public object.\nobject First {\n    // Member description.\n    send(x is int = 2) is void {}\n}\n// Private description.\nhidden object Secret {}\n";
        std::fs::write(&file, text).unwrap();
        let query = "std.demo.";
        let found = std_completions(query, query.len() as u32, &dir).unwrap();
        assert!(!found.iter().any(|item| item.label == "Secret"));
        let first = found.iter().find(|item| item.label == "First").unwrap();
        assert!(first.documentation.contains("Member description"));
        assert!(first.documentation.contains("x is int = 2"));
        let overlay = std::collections::HashMap::from([(file.canonicalize().unwrap(), std::rc::Rc::from(text.replace("Member description", "Unsaved description")))]);
        let found = super::std_completions_with_overlay(query, query.len() as u32, &dir, &overlay).unwrap();
        assert!(found.iter().find(|item| item.label == "First").unwrap().documentation.contains("Unsaved description"));
        let deep = "std.Fir";
        let found = super::std_completions_with_overlay(deep, deep.len() as u32, &dir, &overlay).unwrap();
        assert!(found.iter().find(|item| item.label == "demo.First").unwrap().documentation.contains("Unsaved description"));
        std::fs::write(&file, text.replace("First", "Renamed").replace("Member description", "Saved description")).unwrap();
        let deep = "std.Ren";
        let found = std_completions(deep, deep.len() as u32, &dir).unwrap();
        assert!(found.iter().find(|item| item.label == "demo.Renamed").unwrap().documentation.contains("Saved description"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
