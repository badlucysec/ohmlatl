use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

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

/// Set by SIGINT, SIGTERM or SIGHUP once `catch_signals` ran: long commands stop and clean up.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Without SA_RESTART, so a read from the TTY returns EINTR. Children get the default handlers.
pub fn catch_signals() {
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = on_signal as *const () as usize;
    for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::sigaction(s, &sa, std::ptr::null_mut()) };
    }
}

pub fn check_interrupted() -> Result<(), String> {
    if INTERRUPTED.load(Ordering::SeqCst) {
        return Err("interrupted".into());
    }
    Ok(())
}

/// One line from the controlling terminal, without echo when `secret`, without the newline.
pub fn tty_line(prompt: &str, secret: bool) -> Result<Vec<u8>, String> {
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| format!("/dev/tty: {e}"))?;
    let fd = tty.as_raw_fd();
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if secret {
        if unsafe { libc::tcgetattr(fd, &mut old) } != 0 {
            return last_err("tcgetattr /dev/tty");
        }
        let mut t = old;
        t.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &t) };
    }
    let _ = tty.write_all(prompt.as_bytes());
    let mut line = vec![];
    let mut b = [0u8; 1];
    let r = loop {
        match unsafe { libc::read(fd, b.as_mut_ptr().cast(), 1) } {
            1 if b[0] == b'\n' => break Ok(()),
            1 => line.push(b[0]),
            0 => break Err("end of input on /dev/tty".to_string()),
            _ if INTERRUPTED.load(Ordering::SeqCst) => break Err("interrupted".to_string()),
            _ if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => {}
            _ => break last_err("read /dev/tty"),
        }
    };
    if secret {
        unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &old) };
        let _ = tty.write_all(b"\n");
    }
    r.map(|()| line)
}

/// The rootfs passphrase from the TTY: not empty, no line breaks; asked twice with `confirm`.
pub fn passphrase(prompt: &str, confirm: bool) -> Result<Vec<u8>, String> {
    // Ctrl-C must reach tty_line, which turns echo back on.
    catch_signals();
    loop {
        let p = tty_line(&format!("{prompt}: "), true)?;
        if p.is_empty() || p.contains(&b'\r') {
            eprintln!("the passphrase must not be empty or contain line breaks");
            continue;
        }
        if !confirm || tty_line("Repeat: ", true)? == p {
            return Ok(p);
        }
        eprintln!("the passphrases do not match");
    }
}

/// `yes` from the TTY.
pub fn confirm(question: &str) -> Result<bool, String> {
    Ok(tty_line(&format!("{question} [y/N] "), false)? == b"y")
}

/// A tool from `find_tool`, or an error naming it.
pub fn tool(name: &str) -> Result<PathBuf, String> {
    find_tool(name).ok_or_else(|| format!("{name} not found; see `ohmlatl doctor --build`"))
}

/// Runs `cmd` with `input` on stdin; output goes to the terminal.
pub fn run(cmd: &mut Command, input: Option<&[u8]>) -> Result<(), String> {
    // "sudo -n cryptsetup", "podman unshare": enough to tell which step failed.
    let what: Vec<_> = std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .take(3)
        .map(|a| a.to_string_lossy())
        .collect();
    let what = what.join(" ");
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| format!("{what}: {e}"))?;
    if let Some(data) = input {
        // An error here shows up as the command failing.
        let _ = child.stdin.take().unwrap().write_all(data);
    }
    let s = child.wait().map_err(|e| format!("{what}: {e}"))?;
    if !s.success() {
        return Err(format!("{what} failed ({s})"));
    }
    Ok(())
}

/// `sudo -n <args>`, after `Sudo::start`.
pub fn sudo(args: &[&OsStr], input: Option<&[u8]>) -> Result<(), String> {
    run(Command::new("sudo").arg("-n").args(args), input)
}

/// `sudo -v` once, then `sudo -n -v` every 60 s until dropped, so a long step never asks again.
pub struct Sudo(#[allow(dead_code)] mpsc::Sender<()>);

impl Sudo {
    pub fn start() -> Result<Sudo, String> {
        run(Command::new("sudo").arg("-v"), None)?;
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) =
                rx.recv_timeout(Duration::from_secs(60))
            {
                let _ = Command::new("sudo")
                    .args(["-n", "-v"])
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        });
        Ok(Sudo(tx))
    }
}

/// `flock(LOCK_EX | LOCK_NB)`; false if someone else holds it.
pub fn try_lock(f: &std::fs::File) -> bool {
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
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
