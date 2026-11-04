//! `ohmlatl image ls|inspect|rm|resize|passwd` (docs/TECHNICAL.md, "Imagens").

use crate::build::{LUKS_OFFSET, PBKDF};
use crate::home;
use crate::manifest::{self, parse_size};
use crate::supervise::{valid_name, valid_tag};
use crate::util::{self, check_interrupted, gib, sudo, tool, try_lock};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIB: u64 = 1 << 20;

fn say(msg: impl std::fmt::Display) {
    println!("[ohmlatl] {msg}");
}

fn image_ref(s: &str) -> Result<(&str, &str), String> {
    match s.split_once(':') {
        Some((n, t)) if valid_name(n) && valid_tag(t) => Ok((n, t)),
        _ => Err(format!("invalid image {s} (want <name>:<tag>)")),
    }
}

/// (disk size, bytes on the drive) of a `rootfs.luks`.
fn sizes(m: &fs::Metadata) -> (u64, u64) {
    (m.len().saturating_sub(LUKS_OFFSET), m.blocks() * 512)
}

/// Every `images/<name>/<tag>/`, sorted.
fn all(images: &Path) -> Vec<(String, String, PathBuf)> {
    let names = |d: &Path| -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        v.sort();
        v
    };
    let mut out = vec![];
    for n in names(images) {
        for t in names(&images.join(&n)) {
            let p = images.join(&n).join(&t);
            out.push((n.clone(), t, p));
        }
    }
    out
}

pub fn ls(home_flag: Option<&Path>) -> Result<(), String> {
    let home = home::discover(home_flag)?;
    let rows: Vec<[String; 4]> = all(&home.join("images"))
        .into_iter()
        .map(|(n, t, p)| match fs::metadata(p.join("rootfs.luks")) {
            Ok(m) => {
                let (size, used) = sizes(&m);
                [n, t, gib(size), gib(used)]
            }
            Err(_) => [n, t, "-".into(), "-".into()],
        })
        .collect();
    let head = ["NAME", "TAG", "SIZE", "ON DISK"].map(String::from);
    let w: Vec<usize> = (0..3)
        .map(|i| {
            rows.iter()
                .chain([&head])
                .map(|r| r[i].len())
                .max()
                .unwrap()
        })
        .collect();
    for r in [&head].into_iter().chain(&rows) {
        println!(
            "{:<a$}   {:<b$}   {:<c$}   {}",
            r[0],
            r[1],
            r[2],
            r[3],
            a = w[0],
            b = w[1],
            c = w[2]
        );
    }
    Ok(())
}

pub fn inspect(home_flag: Option<&Path>, r: &str) -> Result<(), String> {
    let (name, tag) = image_ref(r)?;
    let dir = home::discover(home_flag)?
        .join("images")
        .join(name)
        .join(tag);
    let m = fs::metadata(dir.join("rootfs.luks")).map_err(|_| format!("no image {r}"))?;
    let (size, used) = sizes(&m);
    let text = fs::read_to_string(dir.join("manifest.yaml"))
        .map_err(|e| format!("{}: {e}", dir.join("manifest.yaml").display()))?;
    println!("image:   {r}");
    println!("rootfs:  {}", dir.join("rootfs.luks").display());
    println!("size:    {}", gib(size));
    println!("on disk: {}", gib(used));
    println!("\n{}", text.trim_end());
    manifest::parse(&text).map_err(|e| format!("manifest.yaml: {e}"))?;
    Ok(())
}

/// The image directory, locked like the supervisor does: fails while the VM runs.
fn lock(home: &Path, r: &str) -> Result<(PathBuf, File), String> {
    let (name, tag) = image_ref(r)?;
    let dir = home.join("images").join(name).join(tag);
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => {}
        _ => return Err(format!("no image {r}")),
    }
    let f = File::open(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !try_lock(&f) {
        return Err(format!("{r} is in use: stop its VM first"));
    }
    Ok((dir, f))
}

pub fn rm(home_flag: Option<&Path>, r: &str) -> Result<(), String> {
    let home = home::discover(home_flag)?;
    home::check_owner(&home)?;
    let (dir, _lock) = lock(&home, r)?;
    if !util::confirm(&format!(
        "Remove {r} ({})? Its disk and everything in it are deleted.",
        dir.display()
    ))? {
        return Err("not removed".into());
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    say(format!("removed {r}"));
    Ok(())
}

/// `rootfs.luks` must be our own regular file before sudo touches it.
fn rootfs(dir: &Path) -> Result<(PathBuf, fs::Metadata), String> {
    let p = dir.join("rootfs.luks");
    let m = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !m.is_file() || m.uid() != util::uid() {
        return Err(format!(
            "{} must be a regular file owned by uid {}",
            p.display(),
            util::uid()
        ));
    }
    Ok((p, m))
}

/// cryptsetup >= 2.1 and e2fsprogs >= 1.45.
fn check_tools() -> Result<(), String> {
    for (t, flag, min, label) in [
        ("cryptsetup", "--version", (2, 1), "cryptsetup"),
        ("e2fsck", "-V", (1, 45), "e2fsprogs"),
    ] {
        let o = Command::new(tool(t)?)
            .arg(flag)
            .output()
            .map_err(|e| format!("{t}: {e}"))?;
        let text = String::from_utf8_lossy(&o.stdout) + String::from_utf8_lossy(&o.stderr);
        match util::version(&text) {
            Some(v) if v >= min => {}
            _ => return Err(format!("{label} >= {}.{} is required", min.0, min.1)),
        }
    }
    tool("resize2fs").map(drop)
}

pub fn resize(home_flag: Option<&Path>, r: &str, size: &str) -> Result<(), String> {
    let new = parse_size(size)
        .filter(|s| *s > 0)
        .ok_or(format!("invalid size {size} (e.g. 64G)"))?
        .div_ceil(MIB)
        * MIB;
    let home = home::discover(home_flag)?;
    home::check_owner(&home)?;
    let (dir, _lock) = lock(&home, r)?;
    let (luks, meta) = rootfs(&dir)?;
    let (old, used) = sizes(&meta);
    if new == old {
        say(format!("{r} is already {}", gib(new)));
        return Ok(());
    }
    check_tools()?;
    let exe = fs::read_link("/proc/self/exe").map_err(|e| format!("/proc/self/exe: {e}"))?;
    home::check_not_foreign_writable(&exe)?;
    home::check_not_foreign_writable(&home)?;
    say(format!(
        "{r}: {} -> {} ({} on disk)",
        gib(old),
        gib(new),
        gib(used)
    ));
    // The guest gets I/O errors if the drive fills up; the announced sizes are not limited.
    let total: u64 = all(&home.join("images"))
        .iter()
        .filter(|(_, _, p)| *p != dir)
        .filter_map(|(_, _, p)| fs::metadata(p.join("rootfs.luks")).ok())
        .map(|m| sizes(&m).0)
        .sum::<u64>()
        + new;
    let drive = util::statvfs(&home)
        .map(|s| s.f_blocks * s.f_frsize)
        .unwrap_or(u64::MAX);
    if total > drive {
        say(format!(
            "warning: the images add up to {}, more than the drive ({})",
            gib(total),
            gib(drive)
        ));
    }
    let shrink = new < old;
    if shrink
        && !util::confirm(&format!(
            "Shrinking {r} to {}: a power loss in the middle can corrupt its filesystem; back up {} first. Continue?",
            gib(new),
            dir.display()
        ))?
    {
        return Err("not resized".into());
    }
    let pass = util::passphrase("Rootfs passphrase", false)?;
    let _sudo = util::Sudo::start()?;
    let cryptsetup = tool("cryptsetup")?;
    let mapper = format!("ohmlatl-resize-{}", std::process::id());
    let dev = PathBuf::from("/dev/mapper").join(&mapper);
    let set_len = |n: u64| {
        fs::OpenOptions::new()
            .write(true)
            .open(&luks)
            .and_then(|f| f.set_len(n + LUKS_OFFSET))
            .map_err(|e| format!("{}: {e}", luks.display()))
    };

    // Grow: the file first, so the mapper and resize2fs see the whole new disk.
    if !shrink {
        set_len(new)?;
    }
    let opened = sudo(
        &[
            cryptsetup.as_os_str(),
            "open".as_ref(),
            "--key-file=-".as_ref(),
            luks.as_os_str(),
            mapper.as_ref(),
        ],
        Some(&pass),
    );
    let mut resized = false;
    let r2 = opened.and_then(|()| {
        say("e2fsck + resize2fs");
        let r = fs_resize(&dev, shrink.then_some(new), &mut resized);
        let closed = sudo(
            &[cryptsetup.as_os_str(), "close".as_ref(), mapper.as_ref()],
            None,
        );
        r.and(closed)
    });
    match r2 {
        // Shrink: the file last, once ext4 and the mapper are gone.
        Ok(()) if shrink => set_len(new)?,
        Ok(()) => {}
        Err(e) => {
            if !shrink && !resized {
                set_len(old)?;
            }
            return Err(e);
        }
    }
    say("done");
    Ok(())
}

/// `e2fsck -f -p`, then `resize2fs` to the whole device (grow) or to `shrink_to`.
fn fs_resize(dev: &Path, shrink_to: Option<u64>, resized: &mut bool) -> Result<(), String> {
    let e2fsck = tool("e2fsck")?;
    let resize2fs = tool("resize2fs")?;
    check_interrupted()?;
    let s = Command::new("sudo")
        .arg("-n")
        .arg(&e2fsck)
        .args(["-f", "-p"])
        .arg(dev)
        .status()
        .map_err(|e| format!("e2fsck: {e}"))?;
    // 0: clean, 1: errors fixed. Anything else was not fixed by itself.
    if !matches!(s.code(), Some(0 | 1)) {
        return Err(format!(
            "e2fsck could not fix the filesystem ({s}); size unchanged"
        ));
    }
    check_interrupted()?;
    let Some(size) = shrink_to else {
        *resized = true;
        return sudo(&[resize2fs.as_os_str(), dev.as_os_str()], None);
    };
    let o = Command::new("sudo")
        .arg("-n")
        .arg(&resize2fs)
        .arg("-P")
        .arg(dev)
        .output()
        .map_err(|e| format!("resize2fs -P: {e}"))?;
    // "Estimated minimum size of the filesystem: <blocks>", in 4 KiB blocks (mke2fs.conf).
    let min = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .last()
        .and_then(|n| n.parse::<u64>().ok())
        .filter(|_| o.status.success())
        .ok_or("resize2fs -P failed")?
        * 4096;
    if size < min {
        return Err(format!(
            "{} is below the minimum of the filesystem ({}); size unchanged",
            gib(size),
            gib(min)
        ));
    }
    *resized = true;
    sudo(
        &[
            resize2fs.as_os_str(),
            dev.as_os_str(),
            format!("{}M", size / MIB).as_ref(),
        ],
        None,
    )
}

/// A key file in memory, inherited by children as `/proc/self/fd/<n>`; never in argv.
fn memfd(data: &[u8]) -> Result<File, String> {
    let fd = unsafe { libc::memfd_create(c"ohmlatl-key".as_ptr(), 0) };
    if fd < 0 {
        return util::last_err("memfd_create");
    }
    let mut f = unsafe { File::from_raw_fd(fd) };
    f.write_all(data).map_err(|e| format!("memfd: {e}"))?;
    Ok(f)
}

pub fn passwd(home_flag: Option<&Path>, r: &str) -> Result<(), String> {
    let home = home::discover(home_flag)?;
    home::check_owner(&home)?;
    let (dir, _lock) = lock(&home, r)?;
    let (luks, _) = rootfs(&dir)?;
    check_tools()?;
    let cur = memfd(&util::passphrase("Current passphrase", false)?)?;
    let new = memfd(&util::passphrase("New passphrase", true)?)?;
    let fd = |f: &File| format!("/proc/self/fd/{}", f.as_raw_fd());
    let mut cmd = Command::new(tool("cryptsetup")?);
    cmd.arg("luksChangeKey")
        .args(PBKDF)
        .args(["--key-file", &fd(&cur)])
        .arg(&luks)
        .arg(OsStr::new(&fd(&new)));
    util::run(&mut cmd, None)?;
    say(format!("passphrase of {r} changed"));
    Ok(())
}
