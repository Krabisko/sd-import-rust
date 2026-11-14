//! Scanning a card, planning where files go, and copying them safely.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{self, File, FileTimes};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::ui::{c, human, quit, yes};

const RAW_EXT: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "dng", "3fr", "iiq", "rwl",
    "srw", "x3f", "gpr", "erf", "kdc", "mrw", "mos",
];
const JPEG_EXT: &[&str] = &["jpg", "jpeg", "heic", "heif", "hif", "png", "tif", "tiff", "webp"];
const VIDEO_EXT: &[&str] = &[
    "mp4", "mov", "mts", "m2ts", "avi", "mxf", "mkv", "3gp", "m4v", "insv", "360", "braw", "r3d", "crm", "wmv", "mpg",
    "mpeg",
];
/// Card housekeeping that is never worth copying.
const SKIP_DIRS: &[&str] = &["system volume information", "$recycle.bin", ".trashes", ".spotlight-v100", ".fseventsd"];
const SKIP_FILES: &[&str] = &[".ds_store", "thumbs.db", "desktop.ini", "indexervolumeguid"];

const COPY_CHUNK: usize = 8 * 1024 * 1024;
const SAMPLE: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Raw,
    Jpeg,
    Video,
    Other,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub size: u64,
    pub kind: Kind,
}

pub fn classify(path: &Path) -> Kind {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ext = ext.as_str();
    if RAW_EXT.contains(&ext) {
        Kind::Raw
    } else if JPEG_EXT.contains(&ext) {
        Kind::Jpeg
    } else if VIDEO_EXT.contains(&ext) {
        Kind::Video
    } else {
        Kind::Other
    }
}

/// Every importable file under `source`, sorted by path. Hidden files and folders,
/// symlinked folders and card housekeeping are skipped; unreadable entries are ignored.
pub fn scan(source: &Path) -> Vec<SourceFile> {
    fn walk(dir: &Path, files: &mut Vec<SourceFile>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                if !name.starts_with('.') && !SKIP_DIRS.contains(&lower.as_str()) {
                    walk(&path, files);
                }
                continue;
            }
            if name.starts_with('.') || SKIP_FILES.contains(&lower.as_str()) {
                continue;
            }
            let Ok(meta) = fs::metadata(&path) else { continue }; // follows symlinks
            if meta.is_dir() {
                continue;
            }
            files.push(SourceFile { kind: classify(&path), size: meta.len(), path });
        }
    }
    let mut files = Vec::new();
    walk(source, &mut files);
    files.sort_by(|a, b| a.path.as_os_str().as_bytes().cmp(b.path.as_os_str().as_bytes()));
    files
}

pub fn destination_dir(kind: Kind, base: &Path, split_photos: bool) -> PathBuf {
    match kind {
        Kind::Video => base.join("Video"),
        Kind::Other => base.join("Other"),
        Kind::Raw if split_photos => base.join("Photo").join("RAW"),
        Kind::Jpeg if split_photos => base.join("Photo").join("JPEG"),
        _ => base.join("Photo"),
    }
}

fn read_up_to(f: &mut File, n: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(n as usize);
    f.take(n).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Cheap identity check: equal size plus identical first and last MB.
pub fn same_file(a: &Path, b: &Path, size: u64) -> io::Result<bool> {
    let (mut fa, mut fb) = (File::open(a)?, File::open(b)?);
    if read_up_to(&mut fa, SAMPLE)? != read_up_to(&mut fb, SAMPLE)? {
        return Ok(false);
    }
    if size > SAMPLE {
        fa.seek(SeekFrom::End(-(SAMPLE as i64)))?;
        fb.seek(SeekFrom::End(-(SAMPLE as i64)))?;
        return Ok(read_up_to(&mut fa, SAMPLE)? == read_up_to(&mut fb, SAMPLE)?);
    }
    Ok(true)
}

/// `IMG.JPG` -> `IMG_1.JPG`.
fn numbered(target: &Path, n: u32) -> PathBuf {
    let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match target.extension() {
        Some(ext) => format!("{stem}_{n}.{}", ext.to_string_lossy()),
        None => format!("{stem}_{n}"),
    };
    target.with_file_name(name)
}

/// Returns (path, already_there). An identical file on disk means it was already imported.
pub fn unique_target(src: &Path, target: &Path, size: u64, claimed: &HashSet<PathBuf>) -> io::Result<(PathBuf, bool)> {
    let mut candidate = target.to_path_buf();
    let mut n = 1;
    loop {
        if !claimed.contains(&candidate) {
            match fs::metadata(&candidate) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((candidate, false)),
                Err(e) => return Err(e),
                Ok(meta) if meta.len() == size && same_file(src, &candidate, size)? => return Ok((candidate, true)),
                Ok(_) => {}
            }
        }
        candidate = numbered(target, n);
        n += 1;
    }
}

/// Copy via `.name.part`, flush to disk, keep times and permissions, verify the size, rename.
pub fn copy_file(src: &Path, dst: &Path, progress: &mut dyn FnMut(u64)) -> io::Result<()> {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dst.with_file_name(format!(".{name}.part"));
    let result = (|| {
        let src_meta = fs::metadata(src)?;
        let mut fi = File::open(src)?;
        let mut fo = File::create(&tmp)?;
        let mut buf = vec![0u8; COPY_CHUNK];
        loop {
            let n = match fi.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            fo.write_all(&buf[..n])?;
            progress(n as u64);
        }
        fo.flush()?;
        fo.sync_all()?;
        let mut times = FileTimes::new();
        if let Ok(t) = src_meta.accessed() {
            times = times.set_accessed(t);
        }
        if let Ok(t) = src_meta.modified() {
            times = times.set_modified(t);
        }
        fo.set_times(times)?;
        fs::set_permissions(&tmp, src_meta.permissions())?;
        drop(fo);
        if fs::metadata(&tmp)?.len() != src_meta.len() {
            return Err(io::Error::other("size mismatch after copy"));
        }
        fs::rename(&tmp, dst)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Free bytes for unprivileged users on the filesystem holding `path`.
pub fn free_space(path: &Path) -> io::Result<u64> {
    let cpath = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: statvfs fills a zeroed struct from a valid NUL-terminated path.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cpath.as_ptr(), &mut st) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

pub struct Outcome {
    pub copied: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

struct Progress {
    done: u64,
    total: u64,
    start: Instant,
}

impl Progress {
    fn show(&self, name: &str) {
        let elapsed = self.start.elapsed().as_secs_f64().max(0.001);
        let pct = if self.total > 0 { self.done as f64 / self.total as f64 * 100.0 } else { 100.0 };
        let speed = self.done as f64 / elapsed;
        let short: String = name.chars().take(40).collect();
        print!(
            "\r  {pct:5.1}%  {}/{}  {}/s  {short:<40}",
            human(self.done as f64),
            human(self.total as f64),
            human(speed)
        );
        let _ = io::stdout().flush();
    }
}

pub fn run_import(sources: &[PathBuf], base: &Path, library: &Path, dry_run: bool) -> Outcome {
    let mut all = Vec::new();
    for src in sources {
        println!("\nScanning {} ...", c(src.display(), "36"));
        let found = scan(src);
        let bytes: u64 = found.iter().map(|f| f.size).sum();
        println!("  {} files, {}", found.len(), human(bytes as f64));
        all.extend(found);
    }
    let nothing = Outcome { copied: 0, skipped: 0, errors: Vec::new() };
    if all.is_empty() {
        println!("{}", c("Nothing to import.", "33"));
        return nothing;
    }

    let count = |k: Kind| all.iter().filter(|f| f.kind == k).count();
    let (raw, jpeg, video, other) = (count(Kind::Raw), count(Kind::Jpeg), count(Kind::Video), count(Kind::Other));
    let split = raw > 0 && jpeg > 0;
    let total: u64 = all.iter().map(|f| f.size).sum();

    println!("{}", c("\nPlan", "1"));
    println!("  Destination: {}", c(base.display(), "32"));
    if split {
        println!("  Photo/RAW   {raw:>6} files");
        println!("  Photo/JPEG  {jpeg:>6} files");
    } else if raw + jpeg > 0 {
        println!("  Photo       {:>6} files", raw + jpeg);
    }
    if video > 0 {
        println!("  Video       {video:>6} files");
    }
    if other > 0 {
        println!("  Other       {other:>6} files");
    }
    println!("  Total       {}", human(total as f64));

    match free_space(library) {
        Ok(free) if total > free => {
            println!("{}", c(format!("\nNot enough space: need {}, only {} free.", human(total as f64), human(free as f64)), "31"));
            quit(1);
        }
        Ok(_) => {}
        Err(e) => println!("{}", c(format!("\nCould not check free space: {e}"), "33")),
    }

    if dry_run {
        println!("{}", c("\nDry run - nothing copied.", "33"));
        return nothing;
    }
    if !yes("\nStart import?", true) {
        quit(0);
    }

    let mut out = Outcome { copied: 0, skipped: 0, errors: Vec::new() };
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut prog = Progress { done: 0, total, start: Instant::now() };

    for file in &all {
        let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dest_dir = destination_dir(file.kind, base, split);
        if let Err(e) = fs::create_dir_all(&dest_dir) {
            out.errors.push(format!("{}: {e}", dest_dir.display()));
            prog.done += file.size;
            continue;
        }
        let (target, exists) = match unique_target(&file.path, &dest_dir.join(&name), file.size, &claimed) {
            Ok(t) => t,
            Err(e) => {
                out.errors.push(format!("{}: {e}", file.path.display()));
                prog.done += file.size;
                continue;
            }
        };
        claimed.insert(target.clone());
        if exists {
            out.skipped += 1;
            prog.done += file.size;
            prog.show(&name);
            continue;
        }
        let before = prog.done;
        let result = copy_file(&file.path, &target, &mut |n| {
            prog.done += n;
            prog.show(&name);
        });
        match result {
            Ok(()) => out.copied += 1,
            Err(e) => {
                prog.done = before + file.size;
                out.errors.push(format!("{}: {e}", file.path.display()));
            }
        }
    }

    println!();
    let elapsed = prog.start.elapsed().as_secs_f64();
    let colour = if out.errors.is_empty() { "32" } else { "33" };
    println!(
        "{}",
        c(
            format!(
                "\nDone in {elapsed:.0}s: {} copied, {} already present, {} errors.",
                out.copied,
                out.skipped,
                out.errors.len()
            ),
            colour
        )
    );
    for e in &out.errors {
        println!("{}", c(format!("  ! {e}"), "31"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn write(p: &Path, data: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    #[test]
    fn classify_by_extension() {
        assert_eq!(classify(Path::new("IMG_0001.CR2")), Kind::Raw);
        assert_eq!(classify(Path::new("a.jpeg")), Kind::Jpeg);
        assert_eq!(classify(Path::new("DJI_0001.MP4")), Kind::Video);
        assert_eq!(classify(Path::new("DJI_0001.SRT")), Kind::Other);
        assert_eq!(classify(Path::new("README")), Kind::Other);
    }

    #[test]
    fn destinations() {
        let b = Path::new("/b");
        assert_eq!(destination_dir(Kind::Raw, b, true), PathBuf::from("/b/Photo/RAW"));
        assert_eq!(destination_dir(Kind::Jpeg, b, true), PathBuf::from("/b/Photo/JPEG"));
        assert_eq!(destination_dir(Kind::Jpeg, b, false), PathBuf::from("/b/Photo"));
        assert_eq!(destination_dir(Kind::Video, b, true), PathBuf::from("/b/Video"));
        assert_eq!(destination_dir(Kind::Other, b, false), PathBuf::from("/b/Other"));
    }

    #[test]
    fn scan_skips_housekeeping_and_sorts() {
        let t = TempDir::new();
        let card = t.path();
        write(&card.join("DCIM/100CANON/IMG_2.CR2"), b"raw2");
        write(&card.join("DCIM/100CANON/IMG_1.CR2"), b"raw1");
        write(&card.join("DCIM/100CANON/IMG_1.JPG"), b"jpg1");
        write(&card.join(".Trashes/junk.jpg"), b"x");
        write(&card.join("System Volume Information/x"), b"x");
        write(&card.join(".DS_Store"), b"x");
        write(&card.join("Thumbs.db"), b"x");
        write(&card.join("DCIM/.hidden.jpg"), b"x");
        let names: Vec<String> =
            scan(card).iter().map(|f| f.path.strip_prefix(card).unwrap().display().to_string()).collect();
        assert_eq!(names, ["DCIM/100CANON/IMG_1.CR2", "DCIM/100CANON/IMG_1.JPG", "DCIM/100CANON/IMG_2.CR2"]);
    }

    #[test]
    fn same_file_compares_head_and_tail() {
        let t = TempDir::new();
        let size = 3 * SAMPLE as usize;
        let a = vec![7u8; size];
        let mut b = a.clone();
        write(&t.path().join("a"), &a);
        write(&t.path().join("b"), &b);
        assert!(same_file(&t.path().join("a"), &t.path().join("b"), size as u64).unwrap());
        b[size - 1] = 8; // differs in the last MB
        write(&t.path().join("b"), &b);
        assert!(!same_file(&t.path().join("a"), &t.path().join("b"), size as u64).unwrap());
    }

    #[test]
    fn unique_target_detects_duplicates_and_numbers_clashes() {
        let t = TempDir::new();
        let src = t.path().join("card/IMG.JPG");
        write(&src, b"AAAA");
        let target = t.path().join("lib/IMG.JPG");
        let claimed = HashSet::new();

        assert_eq!(unique_target(&src, &target, 4, &claimed).unwrap(), (target.clone(), false));
        write(&target, b"AAAA");
        assert_eq!(unique_target(&src, &target, 4, &claimed).unwrap(), (target.clone(), true));
        write(&target, b"BBBB"); // same size, different content
        assert_eq!(unique_target(&src, &target, 4, &claimed).unwrap(), (t.path().join("lib/IMG_1.JPG"), false));

        let mut claimed = HashSet::new();
        claimed.insert(t.path().join("lib/IMG_1.JPG"));
        assert_eq!(unique_target(&src, &target, 4, &claimed).unwrap(), (t.path().join("lib/IMG_2.JPG"), false));
        assert_eq!(numbered(Path::new("/x/noext"), 3), PathBuf::from("/x/noext_3"));
    }

    #[test]
    fn copy_file_leaves_no_part_and_keeps_mtime() {
        let t = TempDir::new();
        let src = t.path().join("src.bin");
        let data: Vec<u8> = (0..(COPY_CHUNK + 123)).map(|i| (i % 251) as u8).collect();
        write(&src, &data);
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        File::options().write(true).open(&src).unwrap().set_modified(old).unwrap();

        let dst = t.path().join("dst.bin");
        let mut seen = 0;
        copy_file(&src, &dst, &mut |n| seen += n).unwrap();
        assert_eq!(seen, data.len() as u64);
        assert_eq!(fs::read(&dst).unwrap(), data);
        assert!(!t.path().join(".dst.bin.part").exists());
        assert_eq!(fs::metadata(&dst).unwrap().modified().unwrap(), old);
    }

    #[test]
    fn free_space_works() {
        assert!(free_space(Path::new("/")).unwrap() > 0);
        assert!(free_space(Path::new("/definitely/not/here")).is_err());
    }
}
