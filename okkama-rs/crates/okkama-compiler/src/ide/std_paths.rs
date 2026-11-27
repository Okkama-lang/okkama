//! Дополнение путей `std.…` в редакторе (`okkama lsp`, §5.2.1).
//!
//! Путь у курсора читается назад от него: `std.fs.pa|`. Пока сегменты
//! строчные, это каталоги и модули std: предлагаются папки и файлы
//! текущего уровня, а если путь уже назвал модуль (`std.io.stdout.`) — его
//! объявления. Целые пути (`io.stdout`, а с первой набранной буквой и
//! `io.stdout.Stdout`) ищутся по всей std. После enum (`std.compare.Order.`)
//! предлагаются его варианты.
//!
//! The std is a [`StdTree`] (#1115): the std directory on disk for the language server
//! ([`super::DiskStd`], native only), the std bundled into the playground (okkama-wasm). The levels are listed
//! at every request; the parse of every module for whole paths is cached by the tree's [`Stamp`], and modules
//! the editor holds unsaved are parsed again on top of the cache.
use crate::source::SourceFile;
use crate::syntax::{self, ParseMode, ParseOptions, ast};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The std as completion reads it (#1115). A module is named by its path under the std root, segments joined
/// by `/` without `.ok` (`io/stdout`); only modules whose every segment is a module segment (lowercase ASCII,
/// digits, `_`) count.
pub trait StdTree {
    /// The tree's name in the index cache: its directory, or the bundle's root.
    fn key(&self) -> String;

    /// Every module of the tree, in any order.
    fn modules(&self) -> Vec<String>;

    /// What the texts of `modules` were read from: an index built under an equal stamp is reused. `None`
    /// when the tree cannot tell (a file without a modification time), and nothing is cached.
    fn stamp(&self, modules: &[String]) -> Option<Stamp>;

    /// The folders and modules directly in `folder` (its segments; empty: the root): a name and whether it is
    /// a folder.
    fn entries(&self, folder: &[String]) -> Vec<(String, bool)>;

    /// The stored text of `module`, which the stamp describes, under the name its diagnostics would use.
    fn source(&self, module: &str) -> Option<SourceFile>;

    /// The editor's unsaved text of `module`, when it holds one; it comes before the stored text and is never
    /// cached.
    fn unsaved(&self, module: &str) -> Option<SourceFile>;
}

/// What a std index was built from, compared whole: each module with its length and modification time
/// (nanoseconds since the epoch; a bundled tree, which never changes, gives zero).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp(pub Vec<(String, u64, u128)>);

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

/// Предложения для курсора `offset` по std `tree`; `None` — курсор не в пути `std.…`.
pub fn std_completions(text: &str, offset: u32, tree: &dyn StdTree) -> Option<Vec<StdCompletion>> {
    let (segments, partial) = path_at(text, offset as usize)?;
    if let Some(last) = segments.last().filter(|segment| segment.starts_with(char::is_uppercase)) {
        // `std.module.Enum.`: варианты enum.
        let module = segments[..segments.len() - 1].join("/");
        let declarations = declarations(tree, &module);
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
    let mut items = Vec::new();
    // Папки и модули текущего уровня.
    let mut entries: Vec<(String, bool)> =
        tree.entries(&segments).into_iter().filter(|(name, _)| is_segment(name)).collect();
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
                documentation: module_doc(tree, &module),
                label: name,
                kind: StdKind::Module,
                documentation_source: Some((module.clone(), None)),
                deep: false,
            });
        }
    }
    // Путь уже назвал модуль: его объявления.
    if !segments.is_empty() {
        let module = segments.join("/");
        for decl in declarations(tree, &module) {
            items.push(decl.completion(decl.name.clone(), false));
        }
    }
    // Целые пути из глубины: модули, а с набранной буквой — и объявления.
    let prefix = segments.join("/");
    let base = deep_index(tree);
    let mut index = Arc::clone(&base);
    for (position, module) in base.iter().enumerate() {
        if let Some(source) = tree.unsaved(&module.path) {
            let module = &mut Arc::make_mut(&mut index)[position];
            (module.doc, module.declarations) = contents(parsed(source));
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
pub(crate) fn is_segment(name: &str) -> bool {
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

#[derive(Debug)]
struct CachedIndex {
    stamp: Stamp,
    modules: Arc<Vec<Module>>,
}

/// The parse of every module of `tree`, cached until its stamp changes. Unsaved editor texts are applied after
/// this cache and never stored in it.
fn deep_index(tree: &dyn StdTree) -> Arc<Vec<Module>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedIndex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = tree.key();
    let mut modules = tree.modules();
    modules.retain(|module| module.split('/').all(is_segment));
    modules.sort();
    modules.dedup();
    let stamp = tree.stamp(&modules);
    if let Some(stamp) = &stamp
        && let Some(found) = cache.lock().ok().and_then(|cache| {
            cache.get(&key).filter(|found| found.stamp == *stamp).map(|found| Arc::clone(&found.modules))
        })
    {
        return found;
    }
    let index = Arc::new(modules.into_iter().map(|path| {
        // One read and parse per file for both (#1008), of the stored text only.
        let (doc, declarations) = contents(tree.source(&path).and_then(parsed));
        Module { doc, declarations, path }
    }).collect::<Vec<_>>());
    if let Some(stamp) = stamp
        && let Ok(mut cache) = cache.lock()
    {
        cache.insert(key, CachedIndex { stamp, modules: Arc::clone(&index) });
    }
    index
}

/// The overview and the declarations of one parse.
fn contents(parsed: Option<(crate::doc::ModuleDoc, ast::Program)>) -> (String, Vec<Declaration>) {
    match parsed {
        Some((doc, program)) => {
            let declarations = declarations_of(&doc, &program);
            let overview = doc.overview.map(|doc| doc.markdown()).unwrap_or_default();
            (overview, declarations)
        }
        None => (String::new(), Vec::new()),
    }
}

/// Public declarations and documentation come from the canonical model.
fn declarations(tree: &dyn StdTree, module: &str) -> Vec<Declaration> {
    let Some((doc, program)) = file_doc(tree, module) else { return Vec::new(); };
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

/// The current text of `module` (unsaved first), documented.
fn file_doc(tree: &dyn StdTree, module: &str) -> Option<(crate::doc::ModuleDoc, ast::Program)> {
    if module.is_empty() {
        return None;
    }
    parsed(tree.unsaved(module).or_else(|| tree.source(module))?)
}

fn parsed(source: SourceFile) -> Option<(crate::doc::ModuleDoc, ast::Program)> {
    let options = ParseOptions { mode: ParseMode::Production, ..ParseOptions::default() };
    let program = syntax::parse(&source, &options).ok()?;
    Some((crate::doc::ModuleDoc::of(&source, &program), program))
}

fn module_doc(tree: &dyn StdTree, module: &str) -> String {
    file_doc(tree, module).and_then(|(doc, _)| doc.overview).map(|doc| doc.markdown()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{StdKind, std_completions};
    use crate::ide::DiskStd;
    use std::path::PathBuf;

    fn stdlib() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../std")
    }

    fn labels(text: &str) -> Option<Vec<(String, StdKind, bool)>> {
        std_completions(text, text.len() as u32, &DiskStd::new(&stdlib()))
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
        let found = std_completions(query, query.len() as u32, &DiskStd::new(&dir)).unwrap();
        assert!(!found.iter().any(|item| item.label == "Secret"));
        let first = found.iter().find(|item| item.label == "First").unwrap();
        assert!(first.documentation.contains("Member description"));
        assert!(first.documentation.contains("x is int = 2"));
        let overlay = std::collections::HashMap::from([(file.canonicalize().unwrap(), std::rc::Rc::from(text.replace("Member description", "Unsaved description")))]);
        let found = std_completions(query, query.len() as u32, &DiskStd::with_overlay(&dir, &overlay)).unwrap();
        assert!(found.iter().find(|item| item.label == "First").unwrap().documentation.contains("Unsaved description"));
        let deep = "std.Fir";
        let found = std_completions(deep, deep.len() as u32, &DiskStd::with_overlay(&dir, &overlay)).unwrap();
        assert!(found.iter().find(|item| item.label == "demo.First").unwrap().documentation.contains("Unsaved description"));
        std::fs::write(&file, text.replace("First", "Renamed").replace("Member description", "Saved description")).unwrap();
        let deep = "std.Ren";
        let found = std_completions(deep, deep.len() as u32, &DiskStd::new(&dir)).unwrap();
        assert!(found.iter().find(|item| item.label == "demo.Renamed").unwrap().documentation.contains("Saved description"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A std in memory: modules, a stamp the test sets, unsaved texts.
    struct Memory {
        key: String,
        texts: std::cell::RefCell<Vec<(String, String)>>,
        stamp: std::cell::Cell<Option<u128>>,
        unsaved: Vec<(String, String)>,
    }

    impl super::StdTree for Memory {
        fn key(&self) -> String {
            self.key.clone()
        }
        fn modules(&self) -> Vec<String> {
            self.texts.borrow().iter().map(|(module, _)| module.clone()).collect()
        }
        fn stamp(&self, modules: &[String]) -> Option<super::Stamp> {
            let at = self.stamp.get()?;
            Some(super::Stamp(modules.iter().map(|module| (module.clone(), 0, at)).collect()))
        }
        fn entries(&self, folder: &[String]) -> Vec<(String, bool)> {
            assert!(folder.is_empty());
            self.modules().into_iter().map(|module| (module, false)).collect()
        }
        fn source(&self, module: &str) -> Option<crate::source::SourceFile> {
            let text = self.texts.borrow().iter().find(|(name, _)| name == module)?.1.clone();
            crate::source::SourceFile::new(format!("{module}.ok"), text).ok()
        }
        fn unsaved(&self, module: &str) -> Option<crate::source::SourceFile> {
            let text = self.unsaved.iter().find(|(name, _)| name == module)?.1.clone();
            crate::source::SourceFile::new(format!("{module}.ok"), text).ok()
        }
    }

    fn deep(tree: &Memory, typed: &str) -> Vec<String> {
        let text = format!("std.{typed}");
        let found = std_completions(&text, text.len() as u32, tree).unwrap();
        found.into_iter().filter(|item| item.deep).map(|item| item.label).collect()
    }

    /// #1115: the index of whole paths is the tree's, reused while its stamp is equal, rebuilt when it
    /// changes, never kept without a stamp; unsaved texts apply on top of it and never enter it.
    #[test]
    fn the_index_follows_the_trees_stamp() {
        let tree = Memory {
            key: format!("memory-{}", std::process::id()),
            texts: std::cell::RefCell::new(vec![("shapes".into(), "module shapes;\nobject Circle {}\n".into())]),
            stamp: std::cell::Cell::new(Some(1)),
            unsaved: Vec::new(),
        };
        assert_eq!(deep(&tree, "Cir"), ["shapes.Circle"]);
        // A new text under the same stamp: the index is the cached one.
        tree.texts.borrow_mut()[0].1 = "module shapes;\nobject Square {}\n".into();
        assert_eq!(deep(&tree, "Cir"), ["shapes.Circle"]);
        assert!(deep(&tree, "Squ").is_empty());
        // A new stamp: built again.
        tree.stamp.set(Some(2));
        assert_eq!(deep(&tree, "Squ"), ["shapes.Square"]);
        // No stamp: read every time.
        tree.stamp.set(None);
        tree.texts.borrow_mut()[0].1 = "module shapes;\nobject Oval {}\n".into();
        assert_eq!(deep(&tree, "Ova"), ["shapes.Oval"]);
        tree.texts.borrow_mut()[0].1 = "module shapes;\nobject Cone {}\n".into();
        assert_eq!(deep(&tree, "Con"), ["shapes.Cone"]);
        // Unsaved text on top of a cached index, which keeps the stored one.
        tree.stamp.set(Some(3));
        let edited = Memory { unsaved: vec![("shapes".into(), "module shapes;\nobject Star {}\n".into())], ..tree };
        assert_eq!(deep(&edited, "Sta"), ["shapes.Star"]);
        assert!(deep(&edited, "Con").is_empty());
        let saved = Memory { unsaved: Vec::new(), ..edited };
        assert_eq!(deep(&saved, "Con"), ["shapes.Cone"]);
    }
}
