//! The ohmlatl directory on the drive (docs/TECHNICAL.md, "Diretório do ohmlatl").

use crate::util::{self, cstr};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MARKER: &str = ".ohmlatl";
const MARKER_TEXT: &str = "format: 1\n";

/// `--home`, then `OHMLATL_HOME`, then the directory of the executable if it has the marker.
pub fn find(flag: Option<&Path>, env: Option<OsString>, exe: &Path) -> Result<PathBuf, String> {
    if let Some(d) = flag {
        return Ok(d.to_path_buf());
    }
    if let Some(d) = env.filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    if let Some(d) = exe.parent().filter(|d| d.join(MARKER).is_file()) {
        return Ok(d.to_path_buf());
    }
    Err("no ohmlatl directory: run `ohmlatl init <dir>` or pass --home <dir>".into())
}

/// `find` for this process.
pub fn discover(flag: Option<&Path>) -> Result<PathBuf, String> {
    let exe = fs::read_link("/proc/self/exe").map_err(|e| format!("/proc/self/exe: {e}"))?;
    let dir = find(flag, std::env::var_os("OHMLATL_HOME"), &exe)?;
    let dir = fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    match fs::read_to_string(dir.join(MARKER)) {
        Ok(t) if t == MARKER_TEXT => Ok(dir),
        Ok(_) => Err(format!("{}: unknown {MARKER} format", dir.display())),
        Err(_) => Err(format!(
            "{}: not an ohmlatl directory (run `ohmlatl init {0}`)",
            dir.display()
        )),
    }
}

/// Filesystem name for an allowed filesystem, or an error naming the one found.
pub fn filesystem(p: &Path) -> Result<&'static str, String> {
    let s = util::statfs(p).map_err(|e| format!("{}: {e}", p.display()))?;
    match s.f_type {
        0xEF53 => Ok("ext4"),
        0x9123_683E => Ok("btrfs"),
        0x5846_5342 => Ok("xfs"),
        0x4d44 => Err("FAT (no sparse files, no permissions, 4 GiB per file)".into()),
        0x2011_BAB0 => Err("exFAT (no sparse files, no permissions)".into()),
        t => Err(format!("filesystem type {t:#x}; use ext4, btrfs or xfs")),
    }
}

pub fn noexec(p: &Path) -> bool {
    util::statvfs(p).is_ok_and(|s| s.f_flag & libc::ST_NOEXEC != 0)
}

pub fn chown_fix(dir: &Path) -> String {
    format!("sudo chown -R $(id -u):$(id -g) {}", dir.display())
}

/// Refuses before any `sudo`: if someone other than root or us can write `p` or a directory
/// above it, they could swap the binary that is about to run as root.
pub fn check_not_foreign_writable(p: &Path) -> Result<(), String> {
    let uid = util::uid();
    let mut cur = Some(p);
    while let Some(c) = cur {
        let m = fs::metadata(c).map_err(|e| format!("{}: {e}", c.display()))?;
        let mode = m.mode();
        let sticky_dir = m.is_dir() && mode & 0o1000 != 0;
        let foreign = (m.uid() != 0 && m.uid() != uid)
            || (mode & 0o002 != 0 && !sticky_dir)
            || (mode & 0o020 != 0
                && m.gid() != 0
                && !sticky_dir
                && util::group_has_others(m.gid(), uid));
        if foreign {
            return Err(format!(
                "{} is writable by another user (owner {}, group {}, mode {:o}); refusing to use sudo",
                c.display(),
                m.uid(),
                m.gid(),
                mode & 0o7777
            ));
        }
        cur = c.parent();
    }
    Ok(())
}

/// Owner checks shared by `doctor`: the directory, the binary and `images/` belong to us.
pub fn check_owner(dir: &Path) -> Result<(), String> {
    let uid = util::uid();
    for p in [dir.to_path_buf(), dir.join("ohmlatl"), dir.join("images")] {
        match fs::symlink_metadata(&p) {
            Ok(m) if m.uid() != uid => {
                return Err(format!(
                    "{} is owned by uid {}, not {uid}; fix: {}",
                    p.display(),
                    m.uid(),
                    chown_fix(dir)
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
    }
    Ok(())
}

fn say(msg: impl std::fmt::Display) {
    println!("[ohmlatl] {msg}");
}

/// `ohmlatl init <dir>`.
pub fn init(dir: &Path) -> Result<(), String> {
    let dir = std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let parent = dir.parent().ok_or("init needs a directory below /")?;
    let parent = fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let dir = parent.join(dir.file_name().ok_or("bad directory name")?);
    say(format!("directory: {}", dir.display()));

    let probe = if dir.exists() {
        dir.clone()
    } else {
        parent.clone()
    };
    let fs_name = filesystem(&probe).map_err(|e| format!("refusing {}: {e}", probe.display()))?;
    say(format!("filesystem: {fs_name} (ok)"));
    if noexec(&probe) {
        say(
            "mount: noexec (warning: the binary on the drive will not run; use a host ohmlatl with --home)",
        );
    } else {
        say("mount: exec allowed (ok)");
    }

    let (uid, gid) = (util::uid(), util::gid());
    if !dir.exists() {
        let writable = unsafe { libc::access(cstr(&parent).as_ptr(), libc::W_OK) } == 0;
        if writable {
            fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        } else {
            let exe = fs::read_link("/proc/self/exe").map_err(|e| e.to_string())?;
            check_not_foreign_writable(&exe)?;
            check_not_foreign_writable(&parent)?;
            say(format!(
                "{} is not writable by you; creating the directory with sudo",
                parent.display()
            ));
            let ok = Command::new("sudo")
                .args([
                    "install",
                    "-d",
                    "-m",
                    "0755",
                    "-o",
                    &uid.to_string(),
                    "-g",
                    &gid.to_string(),
                ])
                .arg(&dir)
                .status()
                .map_err(|e| format!("sudo: {e}"))?
                .success();
            if !ok {
                return Err(format!("sudo install -d {} failed", dir.display()));
            }
        }
    }
    let m = fs::metadata(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if m.uid() != uid {
        return Err(format!(
            "{} is owned by uid {}, not {uid}; fix: {}",
            dir.display(),
            m.uid(),
            chown_fix(&dir)
        ));
    }
    say(format!("owner: uid {uid} (ok)"));
    set_mode(&dir, 0o755)?;

    let marker = dir.join(MARKER);
    if !marker.exists() {
        write_atomic(&dir, MARKER, MARKER_TEXT.as_bytes(), 0o644)?;
    }
    let images = dir.join("images");
    if !images.exists() {
        fs::create_dir(&images).map_err(|e| format!("{}: {e}", images.display()))?;
    }
    set_mode(&images, 0o700)?;

    let exe = fs::metadata("/proc/self/exe").map_err(|e| e.to_string())?;
    let dest = dir.join("ohmlatl");
    if fs::metadata(&dest).is_ok_and(|d| d.dev() == exe.dev() && d.ino() == exe.ino()) {
        say("ohmlatl binary already in place");
    } else {
        let data = fs::read("/proc/self/exe").map_err(|e| format!("/proc/self/exe: {e}"))?;
        write_atomic(&dir, "ohmlatl", &data, 0o755)?;
        say("copied ohmlatl binary");
    }
    Ok(())
}

fn set_mode(p: &Path, mode: u32) -> Result<(), String> {
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// Temporary file in `dir`, `fsync`, `rename()`.
fn write_atomic(dir: &Path, name: &str, data: &[u8], mode: u32) -> Result<(), String> {
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let err = |e: std::io::Error| format!("{}: {e}", tmp.display());
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .map_err(err)?;
    f.write_all(data).map_err(err)?;
    f.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(err)?;
    f.sync_all().map_err(err)?;
    fs::rename(&tmp, dir.join(name)).map_err(err)?;
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("{}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ohmlatl-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn find_order() {
        let drive = tmpdir("find-drive");
        fs::write(drive.join(MARKER), MARKER_TEXT).unwrap();
        let exe = drive.join("ohmlatl");
        let flag = Path::new("/flag");

        // --home wins over everything.
        assert_eq!(find(Some(flag), Some("/env".into()), &exe).unwrap(), flag);
        // Then OHMLATL_HOME; an empty value does not count.
        assert_eq!(
            find(None, Some("/env".into()), &exe).unwrap(),
            Path::new("/env")
        );
        assert_eq!(find(None, Some("".into()), &exe).unwrap(), drive);
        // Then the directory of the binary, only with the marker.
        assert_eq!(find(None, None, &exe).unwrap(), drive);
        let elsewhere = tmpdir("find-other");
        assert!(
            find(None, None, &elsewhere.join("ohmlatl"))
                .unwrap_err()
                .contains("ohmlatl init")
        );
        fs::remove_dir_all(drive).unwrap();
        fs::remove_dir_all(elsewhere).unwrap();
    }
}
