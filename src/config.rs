//! Config file, category folders and the first-run setup.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::desktop::{desktop_prefs, file_manager_table, ordered_installed, terminal_table, AppTable};
use crate::ui::{ask, c, choose, fail, yes};

pub const DEFAULT_CATEGORIES: [&str; 3] = ["01_personal", "02_clients", "03_events"];
pub const YEAR_FOLDER: &str = "by year";
/// Compared after `_` and `-` become spaces.
const YEAR_FOLDER_ALIASES: [&str; 2] = ["by year", "yearly"];

/// Same JSON shape as the Python sd-import, so both share ~/.config/sd-import/config.json.
/// Unknown keys are kept when saving.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_manager: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Config {
    /// Library and at least one category are needed to import.
    pub fn is_complete(&self) -> bool {
        self.library.as_deref().is_some_and(|l| !l.is_empty())
            && self.categories.as_ref().is_some_and(|c| !c.is_empty())
    }
}

pub fn config_path() -> PathBuf {
    let nonempty = |k: &str| env::var_os(k).filter(|v| !v.is_empty());
    if let Some(p) = nonempty("SD_IMPORT_CONFIG") {
        return PathBuf::from(p);
    }
    let base = nonempty("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    base.join("sd-import").join("config.json")
}

pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/x` -> `/home/me/x`, then make absolute.
pub fn expand_path(raw: &str) -> PathBuf {
    let p = if raw == "~" {
        home()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(raw)
    };
    if p.is_absolute() {
        p
    } else {
        env::current_dir().map(|d| d.join(&p)).unwrap_or(p)
    }
}

pub fn load_config() -> Option<Config> {
    let text = fs::read_to_string(config_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_config(config: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

/// '01_Personal' -> 'personal', for recognising existing folders.
pub fn norm(name: &str) -> String {
    name.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch.is_whitespace() || matches!(ch, '.' | '_' | '-'))
        .replace(['_', '-'], " ")
        .trim()
        .to_lowercase()
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    dirs.sort();
    dirs
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Existing 'by year' / 'yearly' folder in a category, else '<category>/by year'.
pub fn year_root(category_dir: &Path) -> PathBuf {
    sorted_dirs(category_dir)
        .into_iter()
        .find(|d| {
            let n = file_name(d).to_lowercase().replace(['_', '-'], " ");
            YEAR_FOLDER_ALIASES.contains(&n.as_str())
        })
        .unwrap_or_else(|| category_dir.join(YEAR_FOLDER))
}

/// Map each wanted category to an existing top-level folder with the same normalised name.
/// Returns (categories, other existing folders).
pub fn match_categories(library: &Path, wanted: &[String]) -> (Vec<String>, Vec<String>) {
    let existing: Vec<String> = sorted_dirs(library).iter().map(|d| file_name(d)).collect();
    let result: Vec<String> = wanted
        .iter()
        .map(|cat| {
            if existing.contains(cat) {
                cat.clone()
            } else {
                existing.iter().find(|e| norm(e) == norm(cat)).cloned().unwrap_or_else(|| cat.clone())
            }
        })
        .collect();
    let others = existing.into_iter().filter(|e| !result.contains(e)).collect();
    (result, others)
}

fn rel(p: &Path, base: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).display().to_string()
}

fn choose_app(kind: &str, table: &AppTable, preferred: &[String], current: Option<&str>, labels: &[(&str, &str)]) -> String {
    let installed = ordered_installed(table, preferred);
    let mut options: Vec<String> = installed
        .iter()
        .map(|n| {
            let label = labels.iter().find(|(k, _)| k == n).map(|(_, l)| l.to_string()).unwrap_or_else(|| n.clone());
            let suggested = preferred.first() == Some(n);
            if suggested { format!("{label}  (suggested)") } else { label }
        })
        .collect();
    options.push("Other (type a command)".into());
    let default = current.and_then(|cur| installed.iter().position(|n| n == cur)).unwrap_or(0);
    let idx = choose(&format!("Which {kind}?"), &options, Some(default));
    if idx < installed.len() {
        return installed[idx].clone();
    }
    let (hint, what) = if kind == "terminal" { ("{cmd}", "command") } else { ("{path}", "folder") };
    let custom_default = current.filter(|cur| !table.iter().any(|(n, _)| n == cur));
    loop {
        let answer = ask(&format!("Command (use {hint} where the {what} goes)"), custom_default);
        if !answer.is_empty() {
            return answer;
        }
        if let Some(first) = installed.first() {
            return first.clone();
        }
    }
}

fn edit_categories(library: &Path, mut cats: Vec<String>) -> Vec<String> {
    loop {
        println!("{}", c("\nCategories", "1"));
        for cat in &cats {
            let state = if library.join(cat).is_dir() { "found" } else { "will be created" };
            println!("  - {cat}  ({state})");
        }
        let idx = choose("Categories OK?", &["Use these", "Add a category", "Rename one", "Remove one"], Some(0));
        match idx {
            0 if !cats.is_empty() => return cats,
            0 => println!("{}", c("  You need at least one category.", "33")),
            1 => {
                let (_, others) = match_categories(library, &cats);
                if !others.is_empty() {
                    println!("  Other folders in the library: {}", others.join(", "));
                }
                let name = ask("Folder name", None).trim_matches('/').to_string();
                if !name.is_empty() && !cats.contains(&name) {
                    cats.push(name);
                }
            }
            2 if !cats.is_empty() => {
                let i = choose("Rename which?", &cats, None);
                let new = ask("New name", Some(&cats[i])).trim_matches('/').to_string();
                if !new.is_empty() && new != cats[i] {
                    let (old_dir, new_dir) = (library.join(&cats[i]), library.join(&new));
                    if old_dir.is_dir()
                        && !new_dir.exists()
                        && yes(&format!("Also rename the folder on disk to '{new}'?"), false)
                    {
                        if let Err(e) = fs::rename(&old_dir, &new_dir) {
                            println!("{}", c(format!("  ! {}: {e}", old_dir.display()), "31"));
                            continue;
                        }
                    }
                    cats[i] = new;
                }
            }
            3 if !cats.is_empty() => {
                let i = choose("Remove which? (the folder itself is kept)", &cats, None);
                cats.remove(i);
            }
            _ => {}
        }
    }
}

fn is_year(name: &str) -> bool {
    name.len() == 4 && name.bytes().all(|b| b.is_ascii_digit())
}

/// A folder rename, (from, to).
type Move = (PathBuf, PathBuf);

/// Year folders sitting directly in a category: (moves, conflicts).
pub fn find_loose_years(library: &Path, cats: &[String]) -> (Vec<Move>, Vec<Move>) {
    let (mut moves, mut conflicts) = (Vec::new(), Vec::new());
    for cat in cats {
        let cat_dir = library.join(cat);
        let root = year_root(&cat_dir);
        for d in sorted_dirs(&cat_dir) {
            let name = file_name(&d);
            if is_year(&name) {
                let dst = root.join(&name);
                if dst.exists() { conflicts.push((d, dst)) } else { moves.push((d, dst)) }
            }
        }
    }
    (moves, conflicts)
}

/// Offer to move year folders sitting directly in a category into its 'by year' folder.
fn migrate_years(library: &Path, cats: &[String]) {
    let (moves, conflicts) = find_loose_years(library, cats);
    if moves.is_empty() && conflicts.is_empty() {
        return;
    }
    println!("{}", c("\nYear folders outside 'by year'", "1"));
    for (src, dst) in &moves {
        println!("  {}  ->  {}", rel(src, library), rel(dst, library));
    }
    for (src, dst) in &conflicts {
        println!(
            "{}",
            c(format!("  {}: skipped, {} already exists (merge them by hand)", rel(src, library), rel(dst, library)), "33")
        );
    }
    if !moves.is_empty() && yes("Move these? Nothing is copied or deleted, the folders are only renamed.", false) {
        for (src, dst) in &moves {
            match fs::rename(src, dst) {
                Ok(()) => println!("  moved {}", rel(src, library)),
                Err(e) => println!("{}", c(format!("  ! {}: {e}", src.display()), "31")),
            }
        }
    }
}

pub fn setup(config: Option<Config>) -> Config {
    let mut config = config.unwrap_or_default();
    println!("{}", c("SD Import setup", "1;35"));
    let (terms, fms) = desktop_prefs();

    config.terminal = Some(choose_app("terminal", &terminal_table(), &terms, config.terminal.as_deref(), &[]));
    let mut fm_pref = fms;
    fm_pref.push("xdg-open".into());
    config.file_manager = Some(choose_app(
        "file manager",
        &file_manager_table(),
        &fm_pref,
        config.file_manager.as_deref(),
        &[("xdg-open", "System default (xdg-open)")],
    ));

    let default_library = config.library.clone().unwrap_or_else(|| home().join("Pictures").display().to_string());
    let library = loop {
        let raw = ask("\nWhere should imports be saved? (library folder)", Some(&default_library));
        let library = expand_path(&raw);
        if library.is_dir() {
            break library;
        }
        if yes(&format!("{} doesn't exist. Create it?", library.display()), true) {
            match fs::create_dir_all(&library) {
                Ok(()) => break library,
                Err(e) => println!("{}", c(format!("  Could not create it: {e}"), "31")),
            }
        }
    };
    config.library = Some(library.display().to_string());

    let wanted: Vec<String> = config
        .categories
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| DEFAULT_CATEGORIES.iter().map(|s| s.to_string()).collect());
    let (cats, _) = match_categories(&library, &wanted);
    let cats = edit_categories(&library, cats);
    for cat in &cats {
        let root = year_root(&library.join(cat));
        match fs::create_dir_all(&root) {
            Ok(()) => println!("  {} {}", c("ok", "32"), rel(&root, &library)),
            Err(e) => println!("{}", c(format!("  ! {}: {e}", root.display()), "31")),
        }
    }
    config.categories = Some(cats.clone());
    migrate_years(&library, &cats);

    if let Err(e) = save_config(&config) {
        fail(format!("Could not save {}: {e}", config_path().display()));
    }
    println!("{}", c(format!("\nSaved to {}. Run 'sd-import-rust --setup' to change it.\n", config_path().display()), "32"));
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn norm_strips_number_prefix() {
        assert_eq!(norm("01_Personal"), "personal");
        assert_eq!(norm("02-clients"), "clients");
        assert_eq!(norm("Personal"), "personal");
        assert_eq!(norm("3. Events"), "events");
        assert_eq!(norm("by_year"), "by year");
    }

    #[test]
    fn match_categories_reuses_existing_names() {
        let t = TempDir::new();
        for d in ["Personal", "02_clients", "Archive"] {
            fs::create_dir(t.path().join(d)).unwrap();
        }
        let wanted: Vec<String> = DEFAULT_CATEGORIES.iter().map(|s| s.to_string()).collect();
        let (cats, others) = match_categories(t.path(), &wanted);
        assert_eq!(cats, ["Personal", "02_clients", "03_events"]);
        assert_eq!(others, ["Archive"]);
    }

    #[test]
    fn year_root_reuses_alias_folders() {
        let t = TempDir::new();
        let cat = t.path().join("01_personal");
        assert_eq!(year_root(&cat), cat.join("by year"));
        fs::create_dir_all(cat.join("Yearly")).unwrap();
        assert_eq!(year_root(&cat), cat.join("Yearly"));
        let cat2 = t.path().join("02_clients");
        fs::create_dir_all(cat2.join("by_year")).unwrap();
        assert_eq!(year_root(&cat2), cat2.join("by_year"));
    }

    #[test]
    fn loose_years_found_and_conflicts_skipped() {
        let t = TempDir::new();
        fs::create_dir_all(t.path().join("02_clients/2026/Eman")).unwrap();
        fs::create_dir_all(t.path().join("02_clients/2025")).unwrap();
        fs::create_dir_all(t.path().join("02_clients/by year/2025")).unwrap();
        fs::create_dir_all(t.path().join("02_clients/20261")).unwrap();
        let (moves, conflicts) = find_loose_years(t.path(), &["02_clients".into()]);
        assert_eq!(moves, [(t.path().join("02_clients/2026"), t.path().join("02_clients/by year/2026"))]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, t.path().join("02_clients/2025"));
    }

    #[test]
    fn config_json_matches_python_shape() {
        let json = "{\n  \"terminal\": \"cosmic-term\",\n  \"file_manager\": \"xdg-open\",\n  \"library\": \"/mnt/x/Photo & Video\",\n  \"categories\": [\n    \"01_personal\",\n    \"02_clients\"\n  ],\n  \"future\": 1\n}";
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.is_complete());
        assert_eq!(cfg.extra.get("future"), Some(&Value::from(1)));
        assert_eq!(serde_json::to_string_pretty(&cfg).unwrap(), json);
        assert!(!Config::default().is_complete());
    }
}
