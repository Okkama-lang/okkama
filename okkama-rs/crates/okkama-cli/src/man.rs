//! `okkama man` (#512): reference pages from the doc comments of modules, read
//! through the one documentation model of the compiler (`okkama_compiler::doc`).
//!
//!     okkama man                         the index of std
//!     okkama man io/stdout               a module (std/io/stdout too)
//!     okkama man io/stdout Stdout        a declaration
//!     okkama man io/stdout Stdout.write  an action, every overload
//!     okkama man ./src/app.ok            a file of one's own
//!
//! Module paths go through the importer's own search (`FileLoader`): next to
//! the current directory and in its package, then `OKKAMA_IMPORT_DIRS`, then
//! std. A path that names both a module of one's own and a std module is
//! refused as ambiguous. Nothing is executed to read documentation.
//!
//! On a terminal the page is wrapped to its width, headings are bold unless
//! `NO_COLOR` is set, and `$PAGER` shows it when set (run directly, never
//! through a shell). Through a pipe the page is plain text at 80 columns.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use okkama_compiler::doc::{ModuleDoc, publish};
use okkama_compiler::doc::render::{self, IndexEntry, Style};

use super::{EXIT_DIAGNOSTICS, FileLoader, Outcome, locate};

pub(crate) const HELP: &str = "\
okkama man — reference pages from the doc comments of modules

USAGE:
    okkama man [--format text|markdown|man|html|json] [--no-pager]
    okkama man [OPTIONS] MODULE [NAME | NAME.ACTION]
    okkama man [OPTIONS] -k QUERY
    okkama man --all --format man|html|json --output DIRECTORY

MODULE is a module path (`io/stdout`, or `std/io/stdout` for std only) or a
source file (`./src/app.ok`). Without MODULE, the index of std.

OPTIONS:
    --format text      Terminal text (default)
    --format markdown  The page as Markdown
    --format man       A section-3 man(7) page (pipe to man -l -)
    --format html      A standalone static HTML page
    --format json      Documentation model, schema version 1
    --all              Export every std module (no MODULE or -k)
    --output DIRECTORY Write --all output there; man pages go directly here
    --no-pager         Never start $PAGER
    -k QUERY           Search std module paths, public names and summaries
";

#[derive(Copy, Clone, PartialEq, Eq)]
enum Format {
    Text,
    Markdown,
    Man,
    Html,
    Json,
}

pub(crate) fn command(args: &[String]) -> Outcome {
    let mut format = Format::Text;
    let mut pager = true;
    let mut all = false;
    let mut output = None;
    let mut positional = Vec::new();
    let mut query = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let value = match arg.as_str() {
            "-h" | "--help" => return Outcome::success(HELP.to_string()),
            "-k" => {
                if query.is_some() { return Outcome::usage("-k may be given only once"); }
                let Some(value) = rest.next().filter(|value| !value.trim().is_empty()) else {
                    return Outcome::usage("-k needs a non-empty QUERY");
                };
                query = Some(value.trim().to_string());
                continue;
            }
            "--all" => { all = true; continue; }
            "--output" => {
                if output.is_some() { return Outcome::usage("--output may be given only once"); }
                let Some(value) = rest.next().filter(|value| !value.is_empty()) else { return Outcome::usage("--output needs a directory"); };
                output = Some(PathBuf::from(value)); continue;
            }
            "--no-pager" => {
                pager = false;
                continue;
            }
            "--format" => match rest.next() {
                Some(value) => value.as_str(),
                None => return Outcome::usage("--format needs text, markdown, man, html or json"),
            },
            other => match other.strip_prefix("--format=") {
                Some(value) => value,
                None if other.starts_with('-') && other != "-" => {
                    return Outcome::usage(&format!("unknown option `{other}` for `okkama man`"));
                }
                None => {
                    positional.push(other.to_string());
                    continue;
                }
            },
        };
        format = match value {
            "text" => Format::Text,
            "markdown" | "md" => Format::Markdown,
            "man" => Format::Man,
            "html" => Format::Html,
            "json" => Format::Json,
            other => return Outcome::usage(&format!("unknown format `{other}`: text, markdown, man, html or json")),
        };
    }
    if all || output.is_some() {
        if !all || output.is_none() || !positional.is_empty() || query.is_some()
            || !matches!(format, Format::Man | Format::Html | Format::Json) {
            return Outcome::usage("export needs --all --format man|html|json --output DIRECTORY, without MODULE or -k");
        }
    }
    if query.is_some() && !matches!(format, Format::Text | Format::Markdown) {
        return Outcome::usage("-k supports text or markdown");
    }
    let terminal = std::io::stdout().is_terminal();
    let style = Style {
        width: if terminal { columns() } else { 80 },
        ansi: terminal
            && format == Format::Text
            && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
            && std::env::var("TERM").map_or(true, |term| term != "dumb"),
    };
    let mut loader = FileLoader::from_env();
    let mut warnings = String::new();
    if query.is_some() && !positional.is_empty() {
        return Outcome::usage("-k QUERY cannot be combined with a module or declaration");
    }
    let page = if let Some(output) = output {
        export(&mut loader, format, &output, &mut warnings)
    } else if let Some(query) = query {
        search(&loader, &query, format, &style)
    } else { match positional.as_slice() {
        [] => if format == Format::Json { all_documents(&mut loader, &mut warnings).map(|docs| publish::json(&docs)) } else { index(&loader, format, &style) },
        [target] => module_page(&mut loader, target, None, format, &style, &mut warnings),
        [target, name] => module_page(&mut loader, target, Some(name), format, &style, &mut warnings),
        _ => return Outcome::usage("okkama man takes a module and at most one name"),
    }};
    let page = match page {
        Ok(page) => page,
        Err(message) => return Outcome::failure(format!("okkama man: {message}\n"), EXIT_DIAGNOSTICS),
    };
    if terminal
        && pager
        && format == Format::Text
        && let Ok(setting) = std::env::var("PAGER")
        && let Some(()) = show_in_pager(&setting, &page)
    {
        return Outcome { stdout: String::new(), stderr: warnings, code: 0 };
    }
    Outcome { stdout: page, stderr: warnings, code: 0 }
}

/// The terminal's width from `COLUMNS`, 80 when it is not set; pages keep
/// between 40 and 100 columns.
fn columns() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(80)
        .clamp(40, 100)
}

/// Shows `page` through `$PAGER`, split into a program and its arguments
/// without a shell; None when there is no pager or it cannot start.
fn show_in_pager(setting: &str, page: &str) -> Option<()> {
    let mut words = setting.split_whitespace();
    let program = words.next()?;
    let mut child = std::process::Command::new(program)
        .args(words)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    if let Some(mut input) = child.stdin.take() {
        // A pager that quits early closes its input: that is not an error.
        let _ = input.write_all(page.as_bytes());
    }
    let _ = child.wait();
    Some(())
}

/// The index of std, grouped by the first segment of each module path.
fn index(loader: &FileLoader, format: Format, style: &Style) -> Result<String, String> {
    let mut paths = Vec::new();
    collect(&loader.stdlib, &loader.stdlib, &mut paths);
    if paths.is_empty() {
        return Err(format!("no std found in {}", loader.stdlib.display()));
    }
    paths.sort();
    let entries: Vec<IndexEntry> = paths
        .into_iter()
        .map(|path| {
            let summary = ModuleDoc::load(&loader.stdlib.join(format!("{path}.ok")))
                .map(|doc| doc.summary())
                .unwrap_or_default();
            IndexEntry { path, summary }
        })
        .collect();
    Ok(match format {
        Format::Text => render::index_text(&entries, style),
        Format::Markdown => render::index_markdown(&entries),
        Format::Man => publish::index_man(&entries),
        Format::Html => publish::index_html(&entries),
        Format::Json => unreachable!("JSON index loads the full model"),
    })
}

/// Module paths (`io/stdout`) below `dir`.
fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if segment(&name) {
                collect(root, &path, out);
            }
        } else if let Some(stem) = name.strip_suffix(".ok")
            && segment(stem)
            && let Ok(relative) = path.with_extension("").strip_prefix(root)
        {
            let parts: Vec<String> =
                relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
            out.push(parts.join("/"));
        }
    }
}

fn search(loader: &FileLoader, query: &str, format: Format, style: &Style) -> Result<String, String> {
    let mut paths = Vec::new();
    collect(&loader.stdlib, &loader.stdlib, &mut paths);
    if paths.is_empty() { return Err(format!("no std found in {}", loader.stdlib.display())); }
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        let doc = ModuleDoc::load(&loader.stdlib.join(format!("{path}.ok")))?;
        entries.extend(doc.search(query));
    }
    Ok(match format {
        Format::Text => render::search_text(&entries, style),
        Format::Markdown => render::search_markdown(&entries),
        _ => unreachable!("search formats validated before loading"),
    })
}

/// A module path segment: a letter or `_`, then letters, digits and `_`.
pub(super) fn segment(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn module_page(
    loader: &mut FileLoader,
    target: &str,
    name: Option<&String>,
    format: Format,
    style: &Style,
    warnings: &mut String,
) -> Result<String, String> {
    let file = resolve(loader, target)?;
    let doc = load_document(loader, &file, warnings)?;
    let Some(name) = name else {
        return Ok(match format {
            Format::Text => render::module_text(&doc, style),
            Format::Markdown => render::module_markdown(&doc),
            Format::Man => publish::module_man(&doc),
            Format::Html => publish::module_html(&doc),
            Format::Json => publish::json(&[doc]),
        });
    };
    let (item_name, member) = match name.split_once('.') {
        Some((item, member)) => (item, Some(member)),
        None => (name.as_str(), None),
    };
    let Some(item) = doc.item(item_name) else {
        let names: Vec<&str> = doc.items.iter().map(|item| item.name.as_str()).collect();
        return Err(if names.is_empty() {
            format!("{} has no public declarations", doc.path)
        } else {
            format!("{} has no public declaration `{item_name}`; it has {}", doc.path, names.join(", "))
        });
    };
    let members = match member {
        Some(member) => {
            let found = item.members_named(member);
            if found.is_empty() {
                let mut names: Vec<&str> = item.members.iter().map(|member| member.name.as_str()).collect();
                names.dedup();
                return Err(format!(
                    "{} {} has no public member `{member}`{}",
                    doc.path,
                    item.name,
                    if names.is_empty() { String::new() } else { format!("; it has {}", names.join(", ")) }
                ));
            }
            Some(found)
        }
        None => None,
    };
    Ok(match format {
        Format::Text => render::item_text(&doc, item, members.as_deref(), style),
        Format::Markdown => render::item_markdown(&doc, item, members.as_deref()),
        Format::Man | Format::Html | Format::Json => {
            let mut selected = doc.clone();
            let mut item = item.clone();
            if let Some(members) = members {
                item.members = members.into_iter().cloned().collect();
                item.doc = None;
            }
            selected.items = vec![item]; selected.overview = None;
            selected.notes.clear(); selected.link_warnings.clear();
            match format {
                Format::Man => publish::module_man(&selected),
                Format::Html => publish::module_html(&selected),
                _ => publish::json(&[selected]),
            }
        },
    })
}

/// Read through the existing compiler resolver/registry, preserving links and
/// their original source diagnostics for every publishing format.
fn load_document(loader: &mut FileLoader, file: &Path, warnings: &mut String) -> Result<ModuleDoc, String> {
    let text = std::fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let source = okkama_compiler::SourceFile::new(file.display().to_string(), text)
        .map_err(|_| format!("{}: the file is too large", file.display()))?;
    let doc = okkama_compiler::doc::document(&source, loader)?;
    for warning in &doc.link_warnings {
        let location = source.location(warning.span.start);
        warnings.push_str(&format!("{}:{}:{}: warning: unresolved documentation link [{}]\n", source.name(), location.line, location.column, warning.name));
    }
    Ok(doc)
}

fn all_documents(loader: &mut FileLoader, warnings: &mut String) -> Result<Vec<ModuleDoc>, String> {
    let mut paths = Vec::new();
    collect(&loader.stdlib, &loader.stdlib, &mut paths);
    if paths.is_empty() { return Err(format!("no std found in {}", loader.stdlib.display())); }
    paths.sort();
    // This synthetic root imports only discovered std paths. FileLoader's
    // ordinary std-import policy resolves every dependency; one compiler graph
    // supplies every page and all cross-module symbol links.
    let mut text = "module _okkama_doc_export;\n".to_string();
    for (index, path) in paths.iter().enumerate() { text.push_str(&format!("import \"{path}\" as reference_{index};\n")); }
    let source = okkama_compiler::SourceFile::new(loader.stdlib.join(".okkama-doc-export.ok").display().to_string(), text)
        .map_err(|_| "std reference index is too large".to_string())?;
    let mut docs = okkama_compiler::doc::document_graph(&source, loader)?.modules;
    docs.pop(); // The synthetic root is an implementation detail, never exported.
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    let mut names = std::collections::HashSet::new();
    for doc in &docs {
        if !doc.path.split('/').all(segment) || !names.insert(publish::page_name(&doc.path)) {
            return Err(format!("{}: invalid or duplicate documentation page identity {}", doc.file, doc.path));
        }
        let text = std::fs::read_to_string(&doc.file).map_err(|error| format!("{}: {error}", doc.file))?;
        let source = okkama_compiler::SourceFile::new(doc.file.clone(), text).map_err(|_| format!("{}: the file is too large", doc.file))?;
        for warning in &doc.link_warnings {
            let location = source.location(warning.span.start);
            warnings.push_str(&format!("{}:{}:{}: warning: unresolved documentation link [{}]\n", doc.file, location.line, location.column, warning.name));
        }
    }
    Ok(docs)
}

fn export(loader: &mut FileLoader, format: Format, directory: &Path, warnings: &mut String) -> Result<String, String> {
    let docs = all_documents(loader, warnings)?;
    if format == Format::Html && docs.iter().any(|doc| publish::page_name(&doc.path) == "index") {
        return Err("module index conflicts with the HTML index filename".into());
    }
    std::fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let write = |name: &str, content: String| {
        let path = directory.join(name);
        std::fs::write(&path, content).map_err(|error| format!("{}: {error}", path.display()))
    };
    match format {
        Format::Json => {
            write("reference.json", publish::json(&docs))?;
            write("schema-v1.json", publish::JSON_SCHEMA.into())?;
        }
        Format::Html | Format::Man => {
            for doc in &docs {
                let name = publish::page_name(&doc.path);
                if format == Format::Html { write(&format!("{name}.html"), publish::module_html(doc))?; }
                else { write(&format!("{name}.3ok"), publish::module_man(doc))?; }
            }
            if format == Format::Html {
                let entries = docs.iter().map(|doc| IndexEntry { path: doc.path.clone(), summary: doc.summary() }).collect::<Vec<_>>();
                write("index.html", publish::index_html(&entries))?;
            }
        }
        _ => unreachable!("export formats validated before loading"),
    }
    Ok(String::new())
}

/// The source file of `target`: a file path as it is, a module path through
/// the importer's search.
fn resolve(loader: &FileLoader, target: &str) -> Result<PathBuf, String> {
    let as_file = target.ends_with(".ok")
        || target.starts_with("./")
        || target.starts_with("../")
        || Path::new(target).is_absolute();
    if as_file {
        let path = PathBuf::from(target);
        return if path.is_file() { Ok(path) } else { Err(format!("no such file: {target}")) };
    }
    let (std_only, path) = match target.strip_prefix("std/") {
        Some(rest) => (true, rest),
        None => (false, target),
    };
    if path.is_empty() || !path.split('/').all(segment) {
        return Err(format!("`{target}` is not a module path like io/stdout"));
    }
    let file = format!("{path}.ok");
    let in_std = locate(&file, std::slice::from_ref(&loader.stdlib));
    if std_only {
        return in_std.ok_or_else(|| format!("no std module {path} in {}", loader.stdlib.display()));
    }
    let (file, dirs) = loader.candidates(path, "<input>");
    let found = locate(&file, &dirs);
    match (found, in_std) {
        (Some(found), Some(std)) if found != std => Err(format!(
            "{path} is both {} and a std module; say ./{path}.ok or std/{path}",
            found.display()
        )),
        (Some(found), _) => Ok(found),
        (None, _) => Err(format!("no module {path}: not next to the current directory, in its package, the import directories or std")),
    }
}

#[cfg(test)]
mod tests {
    use super::{segment, show_in_pager};

    #[test]
    fn keyword_search_checks_its_arguments_before_loading_std() {
        for args in [&["-k"][..], &["-k", " "], &["-k", "x", "module"], &["-k", "x", "-k", "y"]] {
            let args: Vec<_> = args.iter().map(|arg| arg.to_string()).collect();
            assert_eq!(super::command(&args).code, 2);
        }
        assert!(super::HELP.contains("-k QUERY"));
    }

    #[test]
    fn a_missing_pager_falls_back_to_printing() {
        assert_eq!(show_in_pager("/no/such/pager --flag", "page"), None);
        assert_eq!(show_in_pager("   ", "page"), None);
    }

    #[test]
    fn module_path_segments_are_names() {
        assert!(segment("io") && segment("_x1"));
        assert!(!segment("..") && !segment("") && !segment("1a") && !segment("a-b"));
    }
}
