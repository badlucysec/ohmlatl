use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub fn sha256_reader(mut r: impl Read) -> io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(format!("{:x}", h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

/// `$XDG_CACHE_HOME`, or `~/.cache`.
pub fn cache_home() -> io::Result<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".cache"))
}

pub fn cstr(p: &Path) -> CString {
    CString::new(p.as_os_str().as_bytes()).expect("path with NUL")
}

pub fn last_err<T>(what: impl std::fmt::Display) -> Result<T, String> {
    Err(format!("{what}: {}", io::Error::last_os_error()))
}

pub fn uid() -> u32 {
    unsafe { libc::getuid() }
}

pub fn gid() -> u32 {
    unsafe { libc::getgid() }
}

pub fn statfs(p: &Path) -> io::Result<libc::statfs> {
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cstr(p).as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(s)
}

pub fn statvfs(p: &Path) -> io::Result<libc::statvfs> {
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cstr(p).as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(s)
}

/// Free bytes for an unprivileged user.
pub fn free_bytes(p: &Path) -> io::Result<u64> {
    let s = statvfs(p)?;
    Ok(s.f_bavail * s.f_frsize)
}

pub fn gib(n: u64) -> String {
    format!("{:.1} GiB", n as f64 / (1u64 << 30) as f64)
}

/// A program from `PATH`; tools from `sbin` are also looked up in `/usr/sbin` and `/sbin`,
/// which many distributions leave out of an ordinary user's `PATH`.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([PathBuf::from("/usr/sbin"), PathBuf::from("/sbin")])
        .map(|d| d.join(name))
        .find(|p| {
            let c = cstr(p);
            p.is_file() && unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
        })
}

/// First `x.y` version number in `text`, e.g. "mke2fs 1.47.4 (6-Mar-2025)" -> (1, 47).
pub fn version(text: &str) -> Option<(u32, u32)> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find_map(|w| {
            let mut it = w.split('.');
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
}

/// Name of `uid` in /etc/passwd.
pub fn user_name(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2] == uid.to_string()).then(|| f[0].to_string())
    })
}

/// Whether a user other than `uid` belongs to group `gid` (as primary or supplementary group).
pub fn group_has_others(gid: u32, uid: u32) -> bool {
    let me = user_name(uid).unwrap_or_default();
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let members = group.lines().any(|l| {
        let f: Vec<&str> = l.split(':').collect();
        f.len() > 3 && f[2] == gid.to_string() && f[3].split(',').any(|m| !m.is_empty() && m != me)
    });
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    members
        || passwd.lines().any(|l| {
            let f: Vec<&str> = l.split(':').collect();
            f.len() > 3 && f[3] == gid.to_string() && f[2] != uid.to_string()
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn version() {
        assert_eq!(super::version("mke2fs 1.47.4 (6-Mar-2025)"), Some((1, 47)));
        assert_eq!(super::version("cryptsetup 2.8.8 flags: UDEV"), Some((2, 8)));
        assert_eq!(super::version("no digits"), None);
    }
}
