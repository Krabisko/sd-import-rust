//! Import photos, videos and other files from SD cards into a photo/video library.
//!
//! Layout created:
//!     <library>/<category>/by year/<YYYY>/<YYYY-MM-DD[ name]>/...
//!
//! Inside the import folder:
//!     Photo/RAW, Photo/JPEG   (split only when the card holds both formats)
//!     Photo/                  (otherwise)
//!     Video/
//!     Other/                  (drone telemetry, proxies, sidecars, anything unknown)
//!
//! Nothing is ever deleted from the card. Re-running an import into the same folder
//! skips files that were already copied.

mod cards;
mod config;
mod desktop;
mod import;
mod ui;

use std::env;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, NaiveDate};
use clap::Parser;

use cards::{detect_cards, eject, Card};
use config::{expand_path, load_config, setup, year_root};
use desktop::{open_folder, relaunch_in_terminal};
use import::run_import;
use ui::{ask, c, choose, fail, notify, quit, yes};

#[derive(Parser, Debug)]
#[command(
    name = "sd-import-rust",
    version,
    about = "Import SD card media into a photo/video library.",
    after_help = "The first run asks for your terminal, file manager, library folder and categories and saves\n\
                  them to ~/.config/sd-import/config.json (shared with the Python sd-import).\n\
                  SD_IMPORT_CONFIG=/path/config.json uses a different config file."
)]
struct Args {
    /// Card mount point(s) or folder(s); auto-detected if omitted
    source: Vec<String>,
    /// Show the plan without copying
    #[arg(long)]
    dry_run: bool,
    /// Import date YYYY-MM-DD (default: today)
    #[arg(long, value_name = "YYYY-MM-DD")]
    date: Option<String>,
    /// Choose terminal, file manager, library and categories
    #[arg(long)]
    setup: bool,
    /// Wait for Enter before exiting (keeps a launcher's terminal window open)
    #[arg(long)]
    pause: bool,
    /// Never reopen inside a terminal window, even without one
    #[arg(long)]
    no_terminal: bool,
}

fn pick_sources(given: &[String], library: &Path) -> (Vec<PathBuf>, Vec<Card>) {
    let mut given: Vec<String> = given.to_vec();
    loop {
        if !given.is_empty() {
            let sources: Vec<PathBuf> = given.iter().map(|s| expand_path(s)).collect();
            if let Some(bad) = sources.iter().find(|s| !s.is_dir()) {
                fail(format!("Not a folder: {}", bad.display()));
            }
            return (sources, Vec::new());
        }

        let cards = detect_cards(library);
        if cards.is_empty() {
            println!("{}", c("No SD card / removable drive found.", "33"));
            let path = ask("Insert a card and press Enter to rescan, or type a folder path", None);
            if !path.is_empty() {
                given = vec![path];
            }
            continue;
        }

        let labels: Vec<String> =
            cards.iter().map(|cd| format!("{}  ({}, {})", cd.label, cd.size, cd.mount.display())).collect();
        if cards.len() == 1 {
            println!("Found card: {}", c(&labels[0], "36"));
            return (vec![cards[0].mount.clone()], cards);
        }
        let mut options = labels;
        options.push("All of them".into());
        let idx = choose("Which card?", &options, None);
        let chosen: Vec<Card> = if idx == cards.len() { cards } else { vec![cards[idx].clone()] };
        return (chosen.iter().map(|cd| cd.mount.clone()).collect(), chosen);
    }
}

fn main() {
    let args = Args::parse();

    let config = load_config();
    let has_display = ["WAYLAND_DISPLAY", "DISPLAY"].iter().any(|k| env::var_os(k).is_some_and(|v| !v.is_empty()));
    if !args.no_terminal && has_display && !io::stdin().is_terminal() && !io::stdout().is_terminal() {
        relaunch_in_terminal(config.as_ref());
    }
    if args.pause {
        ui::pause_on_exit();
    }
    let _ = ctrlc::set_handler(|| ui::cancel());

    let today = match &args.date {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .unwrap_or_else(|_| fail(format!("--date must look like 2026-11-09, got '{d}'"))),
        None => Local::now().date_naive(),
    };

    let config = match config {
        Some(cfg) if cfg.is_complete() && !args.setup => cfg,
        other => {
            let cfg = setup(other);
            if args.setup {
                quit(0);
            }
            cfg
        }
    };

    let library = PathBuf::from(config.library.as_deref().unwrap_or_default());
    if !library.is_dir() {
        fail(format!("Library folder not found (is the drive mounted?): {}", library.display()));
    }

    println!("{}", c("SD Import", "1;35"));
    let (sources, cards) = pick_sources(&args.source, &library);

    let cats = config.categories.clone().unwrap_or_default();
    let category = &cats[choose("Import into:", &cats, None)];
    let name = ask("Shoot / client / event name (optional, Enter to skip)", None);
    let date = today.format("%Y-%m-%d").to_string();
    let folder = if name.is_empty() { date } else { format!("{date} {name}") };
    // An existing year folder is reused.
    let base = year_root(&library.join(category)).join(today.year().to_string()).join(folder);

    let outcome = run_import(&sources, &base, &library, args.dry_run);
    if args.dry_run {
        quit(0);
    }

    notify(
        if outcome.errors.is_empty() { "Import finished" } else { "Import finished with errors" },
        &format!("{} copied, {} skipped\n{}", outcome.copied, outcome.skipped, base.display()),
    );

    if !cards.is_empty() && outcome.errors.is_empty() {
        eject(&cards);
    }

    if base.is_dir() && yes("Open the import folder?", false) {
        open_folder(&config, &base);
    }
    quit(0);
}

#[cfg(test)]
mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A throwaway directory under the system temp dir, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir()
                .join(format!("sd-import-rust-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
