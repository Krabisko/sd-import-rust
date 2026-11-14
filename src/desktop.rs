//! Desktop integration: which terminal / file manager to use, reopening in a terminal,
//! and opening the import folder.

use std::env;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::ui::{c, notify};

/// Terminal -> (command prefix, whether the command must be passed as one quoted string).
pub const TERMINALS: &[(&str, &[&str], bool)] = &[
    ("cosmic-term", &["cosmic-term", "-e"], false),
    ("gnome-terminal", &["gnome-terminal", "--"], false),
    ("ptyxis", &["ptyxis", "--"], false),
    ("kgx", &["kgx", "--"], false),
    ("konsole", &["konsole", "-e"], false),
    ("xfce4-terminal", &["xfce4-terminal", "-x"], false),
    ("mate-terminal", &["mate-terminal", "-x"], false),
    ("tilix", &["tilix", "-e"], true),
    ("terminator", &["terminator", "-x"], false),
    ("lxterminal", &["lxterminal", "-e"], true),
    ("qterminal", &["qterminal", "-e"], true),
    ("alacritty", &["alacritty", "-e"], false),
    ("kitty", &["kitty"], false),
    ("foot", &["foot"], false),
    ("wezterm", &["wezterm", "start", "--"], false),
    ("ghostty", &["ghostty", "-e"], false),
    ("xterm", &["xterm", "-e"], false),
    ("xdg-terminal-exec", &["xdg-terminal-exec"], false),
];

pub const FILE_MANAGERS: &[(&str, &[&str])] = &[
    ("xdg-open", &["xdg-open"]),
    ("cosmic-files", &["cosmic-files"]),
    ("nautilus", &["nautilus"]),
    ("dolphin", &["dolphin"]),
    ("nemo", &["nemo"]),
    ("thunar", &["thunar"]),
    ("caja", &["caja"]),
    ("pcmanfm", &["pcmanfm"]),
    ("pcmanfm-qt", &["pcmanfm-qt"]),
];

/// XDG_CURRENT_DESKTOP fragment -> (preferred terminals, preferred file managers).
const DESKTOP_APPS: &[(&str, &[&str], &[&str])] = &[
    ("cosmic", &["cosmic-term"], &["cosmic-files"]),
    ("gnome", &["ptyxis", "kgx", "gnome-terminal"], &["nautilus"]),
    ("ubuntu", &["ptyxis", "gnome-terminal"], &["nautilus"]),
    ("pop", &["cosmic-term", "gnome-terminal"], &["cosmic-files", "nautilus"]),
    ("kde", &["konsole"], &["dolphin"]),
    ("xfce", &["xfce4-terminal"], &["thunar"]),
    ("cinnamon", &["gnome-terminal"], &["nemo"]),
    ("mate", &["mate-terminal"], &["caja"]),
    ("budgie", &["tilix", "gnome-terminal"], &["nautilus", "nemo"]),
    ("lxqt", &["qterminal"], &["pcmanfm-qt"]),
    ("lxde", &["lxterminal"], &["pcmanfm"]),
    ("hyprland", &["kitty", "foot", "alacritty"], &[]),
    ("sway", &["foot", "alacritty", "kitty"], &[]),
    ("niri", &["alacritty", "foot", "kitty"], &[]),
];

/// Name -> binary to look for on PATH.
pub type AppTable = Vec<(&'static str, &'static str)>;

pub fn terminal_table() -> AppTable {
    TERMINALS.iter().map(|(n, p, _)| (*n, p[0])).collect()
}

pub fn file_manager_table() -> AppTable {
    FILE_MANAGERS.iter().map(|(n, p)| (*n, p[0])).collect()
}

/// Full path of an executable, like `shutil.which`.
pub fn which(bin: &str) -> Option<PathBuf> {
    fn executable(p: &Path) -> bool {
        p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    if bin.is_empty() {
        return None;
    }
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return executable(&p).then_some(p);
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(bin))
        .find(|p| executable(p))
}

/// Preferred (terminals, file managers) for a desktop name such as "pop:GNOME".
pub fn desktop_prefs_for(desktop: &str) -> (Vec<String>, Vec<String>) {
    let desktop = desktop.to_lowercase();
    let (mut terms, mut fms) = (Vec::new(), Vec::new());
    for (key, t, f) in DESKTOP_APPS {
        if desktop.contains(key) {
            terms.extend(t.iter().map(|s| s.to_string()));
            fms.extend(f.iter().map(|s| s.to_string()));
        }
    }
    (terms, fms)
}

/// Preferred (terminals, file managers) for the running desktop.
pub fn desktop_prefs() -> (Vec<String>, Vec<String>) {
    desktop_prefs_for(&env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

/// Installed entries of `table`, the desktop's preferred ones first.
pub fn ordered_installed(table: &AppTable, preferred: &[String]) -> Vec<String> {
    let mut names: Vec<&str> = Vec::new();
    for n in preferred.iter().map(String::as_str).chain(table.iter().map(|(n, _)| *n)) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names
        .into_iter()
        .filter_map(|n| table.iter().find(|(name, _)| *name == n))
        .filter(|(_, bin)| which(bin).is_some())
        .map(|(n, _)| n.to_string())
        .collect()
}

fn join(args: &[String]) -> String {
    shlex::try_join(args.iter().map(String::as_str)).unwrap_or_else(|_| args.join(" "))
}

/// Expand a custom command like 'myterm --run {cmd}' with the given argv.
pub fn custom_argv(template: &str, placeholder: &str, args: &[String]) -> Vec<String> {
    let parts = shlex::split(template).unwrap_or_else(|| template.split_whitespace().map(String::from).collect());
    if !template.contains(placeholder) {
        return parts.into_iter().chain(args.iter().cloned()).collect();
    }
    let mut out = Vec::new();
    for p in parts {
        if p == placeholder {
            out.extend(args.iter().cloned());
        } else {
            out.push(p.replace(placeholder, &join(args)));
        }
    }
    out
}

pub fn terminal_argv(choice: &str, cmd: &[String]) -> Vec<String> {
    match TERMINALS.iter().find(|(n, _, _)| *n == choice) {
        Some((_, prefix, joined)) => {
            let mut argv: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
            if *joined {
                argv.push(join(cmd));
            } else {
                argv.extend(cmd.iter().cloned());
            }
            argv
        }
        None => custom_argv(choice, "{cmd}", cmd),
    }
}

/// Configured terminal if usable, else xdg-terminal-exec, the desktop's own, any installed.
pub fn pick_terminal(configured: Option<&str>) -> Option<String> {
    if let Some(choice) = configured.filter(|s| !s.is_empty()) {
        match TERMINALS.iter().find(|(n, _, _)| *n == choice) {
            Some((_, prefix, _)) if which(prefix[0]).is_none() => {}
            _ => return Some(choice.to_string()),
        }
    }
    let (terms, _) = desktop_prefs();
    let preferred: Vec<String> = std::iter::once("xdg-terminal-exec".to_string()).chain(terms).collect();
    ordered_installed(&terminal_table(), &preferred).into_iter().next()
}

/// Started from an app menu without a terminal: reopen ourselves inside one.
pub fn relaunch_in_terminal(config: Option<&Config>) -> ! {
    let Some(term) = pick_terminal(config.and_then(|c| c.terminal.as_deref())) else {
        notify("SD Import needs a terminal", "No terminal emulator found. Run sd-import-rust --setup in one.");
        std::process::exit(1);
    };
    let exe = env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "sd-import-rust".into());
    let mut cmd: Vec<String> = std::iter::once(exe).chain(env::args().skip(1)).collect();
    if !cmd.iter().any(|a| a == "--pause") {
        cmd.push("--pause".into());
    }
    let argv = terminal_argv(&term, &cmd);
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    notify("SD Import could not open a terminal", &format!("{}: {err}", argv[0]));
    std::process::exit(1)
}

pub fn open_folder(config: &Config, path: &Path) {
    let choice = config.file_manager.as_deref().unwrap_or("xdg-open");
    let p = path.to_string_lossy().into_owned();
    let mut argv = match FILE_MANAGERS.iter().find(|(n, _)| *n == choice) {
        Some((_, cmd)) => cmd.iter().map(|s| s.to_string()).chain([p.clone()]).collect(),
        None => custom_argv(choice, "{path}", std::slice::from_ref(&p)),
    };
    if argv.is_empty() || which(&argv[0]).is_none() {
        argv = vec!["xdg-open".into(), p];
    }
    // Own session: otherwise the terminal's hangup on exit kills the opener before the
    // file manager starts (seen on COSMIC, where xdg-open takes the slower generic path).
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Err(e) = command.spawn() {
        println!("{}", c(format!("Could not open the folder: {e}"), "31"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn custom_argv_without_placeholder_appends() {
        assert_eq!(custom_argv("myterm --hold -e", "{cmd}", &s(&["a", "b c"])), s(&["myterm", "--hold", "-e", "a", "b c"]));
    }

    #[test]
    fn custom_argv_with_placeholder() {
        assert_eq!(custom_argv("myterm --run {cmd} --x", "{cmd}", &s(&["a", "b"])), s(&["myterm", "--run", "a", "b", "--x"]));
        // Embedded placeholder becomes one quoted string.
        assert_eq!(custom_argv("sh -c 'exec {cmd}'", "{cmd}", &s(&["a", "b c"])), s(&["sh", "-c", "exec a 'b c'"]));
    }

    #[test]
    fn terminal_argv_known_and_joined() {
        let cmd = s(&["/bin/sd-import-rust", "--pause"]);
        assert_eq!(terminal_argv("cosmic-term", &cmd), s(&["cosmic-term", "-e", "/bin/sd-import-rust", "--pause"]));
        assert_eq!(terminal_argv("wezterm", &cmd), s(&["wezterm", "start", "--", "/bin/sd-import-rust", "--pause"]));
        assert_eq!(terminal_argv("tilix", &cmd), s(&["tilix", "-e", "/bin/sd-import-rust --pause"]));
        let spaced = s(&["/opt/my apps/sd-import-rust"]);
        assert_eq!(terminal_argv("qterminal", &spaced), s(&["qterminal", "-e", "'/opt/my apps/sd-import-rust'"]));
    }

    #[test]
    fn desktop_prefs_match_fragments() {
        let (t, f) = desktop_prefs_for("pop:GNOME");
        assert_eq!(t, s(&["ptyxis", "kgx", "gnome-terminal", "cosmic-term", "gnome-terminal"]));
        assert_eq!(f, s(&["nautilus", "cosmic-files", "nautilus"]));
        assert_eq!(desktop_prefs_for("COSMIC").0, s(&["cosmic-term"]));
        assert!(desktop_prefs_for("").0.is_empty());
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }
}
