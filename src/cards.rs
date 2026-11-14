//! Finding inserted cards (via lsblk) and ejecting them (via udisksctl).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::desktop::which;
use crate::ui::yes;

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub mount: PathBuf,
    pub label: String,
    pub size: String,
    pub device: String,
}

/// lsblk prints booleans as true/false (newer) or "1"/"0" (older).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0) != 0,
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn str_of<'a>(dev: &'a Value, key: &str) -> Option<&'a str> {
    dev.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The library's own drive (a mount that contains the library, or sits inside it) never counts.
fn is_library_drive(mount: &Path, library: &Path) -> bool {
    library.starts_with(mount) || mount.starts_with(library)
}

/// Mounted removable / hot-plugged filesystems from `lsblk -J` output.
pub fn parse_lsblk(json: &str, library: &Path) -> Vec<Card> {
    fn walk(dev: &Value, removable: bool, library: &Path, cards: &mut Vec<Card>) {
        let removable = removable
            || truthy(dev.get("rm"))
            || truthy(dev.get("hotplug"))
            || matches!(str_of(dev, "tran"), Some("usb" | "mmc"));
        let name = str_of(dev, "name").unwrap_or("");
        let mounts = dev.get("mountpoints").and_then(Value::as_array).cloned().unwrap_or_default();
        for mp in mounts.iter().filter_map(Value::as_str) {
            let mount = Path::new(mp);
            if removable && !mp.is_empty() && mp != "/" && !mp.starts_with("/boot") && !is_library_drive(mount, library) {
                cards.push(Card {
                    mount: mount.to_path_buf(),
                    label: str_of(dev, "label").unwrap_or(name).to_string(),
                    size: str_of(dev, "size").unwrap_or("").to_string(),
                    device: str_of(dev, "path").map(String::from).unwrap_or_else(|| format!("/dev/{name}")),
                });
            }
        }
        for child in dev.get("children").and_then(Value::as_array).into_iter().flatten() {
            walk(child, removable, library, cards);
        }
    }

    let Ok(root) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let mut cards = Vec::new();
    for dev in root.get("blockdevices").and_then(Value::as_array).into_iter().flatten() {
        walk(dev, false, library, &mut cards);
    }
    cards
}

pub fn detect_cards(library: &Path) -> Vec<Card> {
    let out = Command::new("lsblk")
        .args(["-J", "-o", "NAME,PATH,RM,HOTPLUG,TRAN,MOUNTPOINTS,LABEL,SIZE"])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_lsblk(&String::from_utf8_lossy(&o.stdout), library),
        _ => Vec::new(),
    }
}

pub fn eject(cards: &[Card]) {
    if which("udisksctl").is_none() {
        println!("Unmount the card in your file manager before removing it.");
        return;
    }
    if !yes("\nEject the card(s)?", true) {
        return;
    }
    for card in cards {
        match Command::new("udisksctl").args(["unmount", "-b", &card.device]).output() {
            Ok(o) => {
                let out = String::from_utf8_lossy(&o.stdout);
                let err = String::from_utf8_lossy(&o.stderr);
                println!("{}", if out.trim().is_empty() { err.trim() } else { out.trim() });
            }
            Err(e) => println!("udisksctl: {e}"),
        }
        let _ = Command::new("udisksctl").args(["power-off", "-b", &card.device]).output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "blockdevices": [
        {"name":"nvme0n1","path":"/dev/nvme0n1","rm":false,"hotplug":false,"tran":"nvme","mountpoints":[null],"label":null,"size":"1.8T",
         "children":[
           {"name":"nvme0n1p1","path":"/dev/nvme0n1p1","rm":false,"hotplug":false,"tran":null,"mountpoints":["/boot"],"label":null,"size":"1G"},
           {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","rm":false,"hotplug":false,"tran":null,"mountpoints":["/"],"label":null,"size":"1.8T"}
         ]},
        {"name":"sda","path":"/dev/sda","rm":false,"hotplug":true,"tran":"usb","mountpoints":[null],"label":null,"size":"7.3T",
         "children":[
           {"name":"sda1","path":"/dev/sda1","rm":false,"hotplug":true,"tran":null,"mountpoints":["/mnt/8tbhdd"],"label":"8tb","size":"7.3T"}
         ]},
        {"name":"mmcblk0","path":"/dev/mmcblk0","rm":"1","hotplug":"1","tran":null,"mountpoints":[null],"label":null,"size":"59.5G",
         "children":[
           {"name":"mmcblk0p1","path":"/dev/mmcblk0p1","rm":"1","hotplug":"1","tran":null,"mountpoints":["/run/media/me/EOS_DIGITAL"],"label":"EOS_DIGITAL","size":"59.5G"}
         ]},
        {"name":"sdb","path":"/dev/sdb","rm":true,"hotplug":true,"tran":"usb","mountpoints":["/run/media/me/disk"],"label":null,"size":"29G"}
      ]
    }"#;

    #[test]
    fn finds_cards_and_skips_library_and_system() {
        let cards = parse_lsblk(SAMPLE, Path::new("/mnt/8tbhdd/8tbhdd/Photo & Video"));
        assert_eq!(
            cards,
            vec![
                Card {
                    mount: "/run/media/me/EOS_DIGITAL".into(),
                    label: "EOS_DIGITAL".into(),
                    size: "59.5G".into(),
                    device: "/dev/mmcblk0p1".into()
                },
                Card { mount: "/run/media/me/disk".into(), label: "sdb".into(), size: "29G".into(), device: "/dev/sdb".into() },
            ]
        );
    }

    #[test]
    fn library_elsewhere_lets_usb_drive_count() {
        let cards = parse_lsblk(SAMPLE, Path::new("/home/me/Pictures"));
        assert_eq!(cards.len(), 3);
        assert_eq!(cards[0].mount, PathBuf::from("/mnt/8tbhdd"));
    }

    #[test]
    fn garbage_is_empty() {
        assert!(parse_lsblk("not json", Path::new("/")).is_empty());
    }
}
