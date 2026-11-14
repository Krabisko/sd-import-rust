//! Terminal prompts, colours, notifications and the exit path (with the optional pause).

use std::fmt::Display;
use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::desktop::which;

static PAUSE_ON_EXIT: AtomicBool = AtomicBool::new(false);
static CANCELLED: AtomicBool = AtomicBool::new(false);

/// Ctrl+C handler (runs on its own thread). With `--pause`, both this thread and a prompt
/// on the main thread may be reading stdin; whichever gets the Enter exits with 130.
pub fn cancel() -> ! {
    println!("{}", c("\nCancelled. Partially copied files end in .part and can be deleted.", "33"));
    if PAUSE_ON_EXIT.load(Ordering::SeqCst) {
        CANCELLED.store(true, Ordering::SeqCst);
        pause();
    }
    let _ = io::stdout().flush();
    std::process::exit(130)
}

/// Make every later `quit` wait for Enter first (keeps a launcher's terminal window open).
pub fn pause_on_exit() {
    PAUSE_ON_EXIT.store(true, Ordering::SeqCst);
}

/// Exit the program, pausing first if `--pause` was given. Every exit goes through here.
pub fn quit(code: i32) -> ! {
    let _ = io::stdout().flush();
    if PAUSE_ON_EXIT.swap(false, Ordering::SeqCst) {
        pause();
    }
    std::process::exit(code)
}

/// Print a message to stderr and exit with status 1.
pub fn fail(msg: impl Display) -> ! {
    eprintln!("{msg}");
    quit(1)
}

/// "Press Enter to close." Reads fd 0 directly so it also works from the Ctrl+C handler
/// while the main thread holds the stdin lock.
fn pause() {
    print!("\nPress Enter to close.");
    let _ = io::stdout().flush();
    let mut byte = 0u8;
    loop {
        // SAFETY: reading one byte into a valid, owned buffer.
        let n = unsafe { libc::read(0, (&mut byte as *mut u8).cast(), 1) };
        if n <= 0 || byte == b'\n' {
            break;
        }
    }
}

/// Wrap `text` in an ANSI colour code, only when stdout is a terminal.
pub fn c(text: impl Display, code: &str) -> String {
    if io::stdout().is_terminal() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Ask a question; Enter returns `default` (or ""). End of input exits.
pub fn ask(prompt: &str, default: Option<&str>) -> String {
    let default = default.filter(|d| !d.is_empty());
    match default {
        Some(d) => print!("{prompt} [{d}]: "),
        None => print!("{prompt}: "),
    }
    let _ = io::stdout().flush();
    let mut line = String::new();
    let read = io::stdin().read_line(&mut line);
    if CANCELLED.load(Ordering::SeqCst) {
        std::process::exit(130);
    }
    match read {
        Ok(0) | Err(_) => {
            println!();
            quit(1)
        }
        Ok(_) => {}
    }
    let answer = line.trim();
    if answer.is_empty() {
        default.unwrap_or("").to_string()
    } else {
        answer.to_string()
    }
}

pub fn yes(prompt: &str, default: bool) -> bool {
    let (hint, d) = if default { ("Y/n", "y") } else { ("y/N", "n") };
    let answer = ask(&format!("{prompt} ({hint})"), Some(d)).to_lowercase();
    answer == "y" || answer == "yes"
}

/// Numbered menu; returns the chosen index. Enter picks `default` (an index) if given.
pub fn choose<S: AsRef<str>>(prompt: &str, options: &[S], default: Option<usize>) -> usize {
    println!("{}", c(prompt, "1"));
    for (i, label) in options.iter().enumerate() {
        println!("  {}) {}", c(i + 1, "36"), label.as_ref());
    }
    let default = default.map(|d| (d + 1).to_string());
    loop {
        let answer = ask("Choose", default.as_deref());
        if let Ok(n) = answer.parse::<usize>() {
            if (1..=options.len()).contains(&n) {
                return n - 1;
            }
        }
        println!("{}", c("  Please type one of the numbers above.", "33"));
    }
}

/// 1536 -> "1.5 KB".
pub fn human(n: f64) -> String {
    let mut n = n;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if n < 1024.0 || unit == "TB" {
            return if unit == "B" { format!("{} B", n.round() as u64) } else { format!("{n:.1} {unit}") };
        }
        n /= 1024.0;
    }
    unreachable!()
}

pub fn notify(summary: &str, body: &str) {
    if which("notify-send").is_some() {
        let _ = Command::new("notify-send")
            .args(["-a", "SD Import", "-i", "camera-photo", summary, body])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human(0.0), "0 B");
        assert_eq!(human(1023.0), "1023 B");
        assert_eq!(human(1536.0), "1.5 KB");
        assert_eq!(human(5.0 * 1024.0 * 1024.0 * 1024.0), "5.0 GB");
        assert_eq!(human(3.0 * 1024f64.powi(5)), "3072.0 TB");
    }
}
