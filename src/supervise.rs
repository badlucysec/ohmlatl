//! The VM supervisor (docs/TECHNICAL.md, "Run e boot", "Stop").
//!
//! P0: run by hand as root, in the foreground. It also does the `launch` steps (tmpfs on
//! /run/ohmlatl, extraction). The TAP is created by hand and passed with --tap/--guest/--gw.
//! P2 moves this behind `launch` + `systemd-run` with managed networking.

use crate::assets;
use crate::util::{cstr, last_err};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RUN: &str = "/run/ohmlatl";
const LOG_MAX: u64 = 1 << 20;
const REPLAY: usize = 64 << 10;
const STOP_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Args {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub name: String,
    pub tag: String,
    pub vcpus: u32,
    pub memory_mib: u32,
    pub tap: String,
    pub guest: Ipv4Addr,
    pub gw: Ipv4Addr,
}

pub fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=31).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub fn valid_tag(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(c))
}

/// The kernel command line: only console, reboot, panic and the static IP.
pub fn cmdline(guest: Ipv4Addr, gw: Ipv4Addr, name: &str) -> String {
    format!("console=ttyS0 reboot=k panic=1 ip={guest}::{gw}:255.255.255.252:{name}:eth0:off")
}

enum Event {
    Exited(io::Result<std::process::ExitStatus>),
    Stop,
}

/// Serial output: a 1 MiB ring in memory, mirrored to serial.log, and the console client.
struct Serial {
    ring: VecDeque<u8>,
    log: File,
    log_len: u64,
    client: Option<UnixStream>,
}

impl Serial {
    fn push(&mut self, data: &[u8]) {
        self.ring.extend(data);
        let excess = self.ring.len().saturating_sub(LOG_MAX as usize);
        self.ring.drain(..excess);
        if self.log_len + data.len() as u64 > LOG_MAX {
            // Keep the file under 1 MiB: restart it from the newest half of the ring.
            let keep: Vec<u8> = self
                .ring
                .iter()
                .skip(self.ring.len().saturating_sub(LOG_MAX as usize / 2))
                .copied()
                .collect();
            let _ = self
                .log
                .set_len(0)
                .and_then(|_| self.log.rewind())
                .and_then(|_| self.log.write_all(&keep));
            self.log_len = keep.len() as u64;
        } else if self.log.write_all(data).is_ok() {
            self.log_len += data.len() as u64;
        }
        if let Some(c) = &mut self.client
            && c.write_all(data).is_err()
        {
            self.client = None;
        }
    }
}

pub fn run(a: Args) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("supervise must run as root (sudo)".into());
    }
    if !valid_name(&a.name) || !valid_tag(&a.tag) {
        return Err(format!("invalid image {}:{}", a.name, a.tag));
    }
    if !a
        .tap
        .strip_prefix("ohm")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err(format!("invalid TAP name {}", a.tap));
    }
    // Signals go to one thread (sigwait); the jailer gets them unblocked again in pre_exec.
    let mut sigs: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut sigs);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut sigs, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &sigs, std::ptr::null_mut());
    }

    let bin = prepare_run()?;
    let id = format!("{}-{}", a.uid, a.name);
    let vmdir = Path::new(RUN).join(a.uid.to_string()).join(&a.name);
    for d in [vmdir.parent().unwrap(), &vmdir] {
        mkdir_owned(d, 0o700, a.uid, a.gid)?;
    }

    // The drive is not trusted by root: resolve everything below --home without symlinks.
    let home = open_dir(&a.home)?;
    let image = openat2(
        &home,
        &format!("images/{}/{}", a.name, a.tag),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    if unsafe { libc::flock(image.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!("image {}:{} is in use", a.name, a.tag));
    }
    let rootfs = openat2(&image, "rootfs.luks", libc::O_RDWR)?;
    let st = fstat(&rootfs)?;
    if st.st_mode & libc::S_IFMT != libc::S_IFREG || st.st_uid != a.uid {
        return Err(format!(
            "rootfs.luks must be a regular file owned by uid {}",
            a.uid
        ));
    }

    let jail = Path::new(RUN).join("jail/firecracker").join(&id);
    let root = jail.join("root");
    cleanup(&jail, &vmdir);
    let result = boot(&a, &bin, &root, &vmdir, &rootfs);
    cleanup(&jail, &vmdir);
    drop(image);
    result
}

fn boot(a: &Args, bin: &Path, root: &Path, vmdir: &Path, rootfs: &OwnedFd) -> Result<(), String> {
    let (master, slave) = pty()?;

    fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    bind_file(&bin.join("vmlinux"), &root.join("vmlinux"), true)?;
    bind_file(
        &bin.join("initramfs.cpio.gz"),
        &root.join("initramfs.cpio.gz"),
        true,
    )?;
    bind_file(
        &PathBuf::from(format!("/proc/self/fd/{}", rootfs.as_raw_fd())),
        &root.join("rootfs.luks"),
        false,
    )?;
    mkdir_owned(&root.join("run"), 0o755, a.uid, a.gid)?;
    let fc_log = root.join("run/firecracker.log");
    File::create(&fc_log).map_err(|e| format!("{}: {e}", fc_log.display()))?;
    chown(&fc_log, a.uid, a.gid, 0o600)?;

    // The unit's cgroup in P2; here, whatever cgroup the supervisor runs in.
    let cgroup = fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
    let parent = cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::/"))
        .ok_or("no cgroup v2 entry in /proc/self/cgroup")?;
    let stdio =
        |fd: &OwnedFd| -> Result<OwnedFd, String> { fd.try_clone().map_err(|e| e.to_string()) };
    let mut cmd = Command::new(bin.join("jailer"));
    cmd.args(["--id", &format!("{}-{}", a.uid, a.name)])
        .arg("--exec-file")
        .arg(bin.join("firecracker"))
        .args(["--uid", &a.uid.to_string(), "--gid", &a.gid.to_string()])
        .args(["--chroot-base-dir", &format!("{RUN}/jail")])
        .args(["--cgroup-version", "2", "--parent-cgroup", parent.trim()])
        .args([
            "--",
            "--api-sock",
            "/run/api.sock",
            "--log-path",
            "/run/firecracker.log",
            "--level",
            "Warning",
        ])
        .stdin(stdio(&slave)?)
        .stdout(stdio(&slave)?)
        .stderr(stdio(&slave)?);
    unsafe {
        cmd.pre_exec(|| {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &all, std::ptr::null_mut());
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| format!("jailer: {e}"))?;
    drop(slave);
    let pid = child.id() as i32;
    eprintln!("supervise: jailer started (pid {pid})");

    let (tx, rx) = mpsc::channel::<Event>();
    spawn_waiter(child, tx.clone());
    follow_log(fc_log);

    // Copy the serial before InstanceStart so no early output is lost.
    let log_path = vmdir.join("serial.log");
    let log = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    chown(&log_path, a.uid, a.gid, 0o600)?;
    let serial = Arc::new(Mutex::new(Serial {
        ring: VecDeque::new(),
        log,
        log_len: 0,
        client: None,
    }));
    let master_w = File::from(master.try_clone().map_err(|e| e.to_string())?);
    pump_pty(File::from(master), serial.clone());

    let api_sock = root.join("run/api.sock");
    let configured = configure(a, &api_sock);
    if let Err(e) = configured {
        unsafe { libc::kill(pid, libc::SIGKILL) };
        let _ = rx.recv_timeout(Duration::from_secs(5));
        return Err(e);
    }

    fs::write(vmdir.join("ip"), format!("{}\n", a.guest)).map_err(|e| e.to_string())?;
    chown(&vmdir.join("ip"), a.uid, a.gid, 0o644)?;
    let console = listen(&vmdir.join("console.sock"), a.uid, a.gid)?;
    let ctl = listen(&vmdir.join("ctl.sock"), a.uid, a.gid)?;
    serve_console(console, serial, master_w);
    serve_ctl(ctl, tx.clone());
    std::thread::spawn(move || {
        let mut sig = 0;
        let mut all: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut all);
            for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                libc::sigaddset(&mut all, s);
            }
        }
        while unsafe { libc::sigwait(&all, &mut sig) } == 0 {
            if tx.send(Event::Stop).is_err() {
                break;
            }
        }
    });
    eprintln!(
        "supervise: ready: {}:{} on {} (guest {}, gateway {})",
        a.name, a.tag, a.tap, a.guest, a.gw
    );
    eprintln!(
        "supervise: console: {}",
        vmdir.join("console.sock").display()
    );

    let mut deadline: Option<Instant> = None;
    loop {
        let ev = match deadline {
            None => rx.recv().map_err(|_| "event channel closed".to_string())?,
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(ev) => ev,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!(
                        "supervise: Firecracker did not exit in {}s, killing it",
                        STOP_TIMEOUT.as_secs()
                    );
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                    deadline = None;
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            },
        };
        match ev {
            Event::Exited(s) => {
                eprintln!(
                    "supervise: Firecracker exited: {}",
                    s.map_or_else(|e| e.to_string(), |s| s.to_string())
                );
                return Ok(());
            }
            Event::Stop if deadline.is_none() => {
                eprintln!("supervise: stop: sending Ctrl-Alt-Del");
                deadline = Some(Instant::now() + STOP_TIMEOUT);
                if let Err(e) = api(
                    &api_sock,
                    "/actions",
                    r#"{"action_type": "SendCtrlAltDel"}"#,
                ) {
                    eprintln!("supervise: SendCtrlAltDel: {e}; killing Firecracker");
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
            Event::Stop => {}
        }
    }
}

fn configure(a: &Args, sock: &Path) -> Result<(), String> {
    let t0 = Instant::now();
    while !sock.exists() {
        if t0.elapsed() > Duration::from_secs(10) {
            return Err(format!("{} did not appear", sock.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let boot = cmdline(a.guest, a.gw, &a.name);
    api(
        sock,
        "/boot-source",
        &format!(
            r#"{{"kernel_image_path": "/vmlinux", "initrd_path": "/initramfs.cpio.gz", "boot_args": "{boot}"}}"#
        ),
    )?;
    api(
        sock,
        "/drives/rootfs",
        r#"{"drive_id": "rootfs", "path_on_host": "/rootfs.luks", "is_root_device": false, "is_read_only": false, "cache_type": "Writeback"}"#,
    )?;
    api(
        sock,
        "/network-interfaces/eth0",
        &format!(r#"{{"iface_id": "eth0", "host_dev_name": "{}"}}"#, a.tap),
    )?;
    api(
        sock,
        "/machine-config",
        &format!(
            r#"{{"vcpu_count": {}, "mem_size_mib": {}}}"#,
            a.vcpus, a.memory_mib
        ),
    )?;
    api(sock, "/entropy", "{}")?;
    api(sock, "/actions", r#"{"action_type": "InstanceStart"}"#)
}

/// One `PUT` on the Firecracker API socket.
fn api(sock: &Path, path: &str, body: &str) -> Result<(), String> {
    let err = |e: io::Error| format!("PUT {path}: {e}");
    let mut s = UnixStream::connect(sock).map_err(err)?;
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(err)?;
    write!(s, "PUT {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).map_err(err)?;
    let mut resp = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf).map_err(err)?;
        if n == 0 {
            break;
        }
        resp.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&resp);
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if resp.len() >= end + 4 + len {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&resp);
    let status = text.split_whitespace().nth(1).unwrap_or("");
    if status.starts_with('2') {
        Ok(())
    } else {
        Err(format!(
            "PUT {path}: {}",
            text.split("\r\n\r\n").nth(1).unwrap_or(&text).trim()
        ))
    }
}

/// `launch` steps 1 and 2: our own tmpfs on /run/ohmlatl, then the binaries in it.
fn prepare_run() -> Result<PathBuf, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open("/run/ohmlatl.lock")
        .map_err(|e| format!("/run/ohmlatl.lock: {e}"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return last_err("flock /run/ohmlatl.lock");
    }
    let mounts = fs::read_to_string("/proc/self/mountinfo").map_err(|e| e.to_string())?;
    if !mounts.lines().any(|l| l.split(' ').nth(4) == Some(RUN)) {
        fs::create_dir_all(RUN).map_err(|e| format!("{RUN}: {e}"))?;
        // No nodev/noexec: the jailer creates device nodes and runs Firecracker in the chroot.
        let r = unsafe {
            libc::mount(
                c"ohmlatl".as_ptr(),
                cstr(Path::new(RUN)).as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_NOSUID,
                c"mode=0755".as_ptr().cast(),
            )
        };
        if r != 0 {
            return last_err(format!("mount tmpfs on {RUN}"));
        }
    }
    let bin =
        assets::extract_run(Path::new(RUN)).map_err(|e| format!("extracting binaries: {e}"))?;
    Ok(bin)
}

/// Kills nothing (the jailer child is gone or killed by then); unmounts and removes the chroot
/// and the sockets. Idempotent.
fn cleanup(jail: &Path, vmdir: &Path) {
    let root = jail.join("root");
    for f in ["vmlinux", "initramfs.cpio.gz", "rootfs.luks"] {
        let p = root.join(f);
        unsafe { libc::umount2(cstr(&p).as_ptr(), libc::MNT_DETACH) };
    }
    let mounts = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let still = mounts.lines().any(|l| {
        l.split(' ')
            .nth(4)
            .is_some_and(|m| Path::new(m).starts_with(jail))
    });
    if still {
        eprintln!(
            "supervise: {} still has mounts; not removing it",
            jail.display()
        );
    } else if jail.exists()
        && let Err(e) = fs::remove_dir_all(jail)
    {
        eprintln!("supervise: {}: {e}", jail.display());
    }
    for f in ["ctl.sock", "console.sock", "ip"] {
        let _ = fs::remove_file(vmdir.join(f));
    }
}

fn spawn_waiter(mut child: Child, tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        let _ = tx.send(Event::Exited(child.wait()));
    });
}

/// Firecracker's own log stays out of the serial: copy it to our stderr (the journal in P2).
fn follow_log(path: PathBuf) {
    std::thread::spawn(move || {
        let Ok(mut f) = File::open(&path) else { return };
        let mut buf = String::new();
        loop {
            buf.clear();
            match f.read_to_string(&mut buf) {
                Ok(0) => std::thread::sleep(Duration::from_millis(500)),
                Ok(_) => buf.lines().for_each(|l| eprintln!("firecracker: {l}")),
                Err(_) => return,
            }
        }
    });
}

fn pump_pty(mut master: File, serial: Arc<Mutex<Serial>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        // EIO once Firecracker closes the slave.
        while let Ok(n @ 1..) = master.read(&mut buf) {
            serial.lock().unwrap().push(&buf[..n]);
        }
    });
}

/// One console client at a time; it first gets the last 64 KiB of the serial.
fn serve_console(l: UnixListener, serial: Arc<Mutex<Serial>>, master: File) {
    let master = Arc::new(Mutex::new(master));
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let mut s = serial.lock().unwrap();
            if s.client.is_some() {
                let mut c = c;
                let _ = c.write_all(b"ohmlatl: another console client is attached\r\n");
                continue;
            }
            let start = s.ring.len().saturating_sub(REPLAY);
            let replay: Vec<u8> = s.ring.iter().skip(start).copied().collect();
            let mut out = c.try_clone().unwrap();
            if out.write_all(&replay).is_err() {
                continue;
            }
            s.client = Some(out);
            drop(s);
            let (serial, master) = (serial.clone(), master.clone());
            std::thread::spawn(move || {
                let mut c = c;
                let mut buf = [0u8; 1024];
                while let Ok(n @ 1..) = c.read(&mut buf) {
                    if master.lock().unwrap().write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                serial.lock().unwrap().client = None;
            });
        }
    });
}

/// P0 control socket: `stop` only.
fn serve_ctl(l: UnixListener, tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut line = String::new();
            let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = (&c).take(64).read_to_string(&mut line);
            let reply = match line.trim() {
                "stop" => {
                    let _ = tx.send(Event::Stop);
                    "ok\n"
                }
                _ => "error: unknown command\n",
            };
            let _ = c.write_all(reply.as_bytes());
        }
    });
}

fn listen(path: &Path, uid: u32, gid: u32) -> Result<UnixListener, String> {
    let _ = fs::remove_file(path);
    let l = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    chown(path, uid, gid, 0o600)?;
    Ok(l)
}

fn pty() -> Result<(OwnedFd, OwnedFd), String> {
    let (mut m, mut s): (RawFd, RawFd) = (-1, -1);
    if unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    } != 0
    {
        return last_err("openpty");
    }
    let (m, s) = unsafe { (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
    // Raw slave: the host line discipline never echoes what the console client types.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    unsafe {
        libc::tcgetattr(s.as_raw_fd(), &mut t);
        libc::cfmakeraw(&mut t);
        if libc::tcsetattr(s.as_raw_fd(), libc::TCSANOW, &t) != 0 {
            return last_err("tcsetattr");
        }
        for fd in [m.as_raw_fd(), s.as_raw_fd()] {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Ok((m, s))
}

fn bind_file(src: &Path, dst: &Path, ro: bool) -> Result<(), String> {
    File::create(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let (s, d) = (cstr(src), cstr(dst));
    unsafe {
        if libc::mount(
            s.as_ptr(),
            d.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        ) != 0
        {
            return last_err(format!("bind mount {} on {}", src.display(), dst.display()));
        }
        if ro
            && libc::mount(
                std::ptr::null(),
                d.as_ptr(),
                std::ptr::null(),
                libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY,
                std::ptr::null(),
            ) != 0
        {
            return last_err(format!("remount {} read-only", dst.display()));
        }
    }
    Ok(())
}

fn mkdir_owned(p: &Path, mode: u32, uid: u32, gid: u32) -> Result<(), String> {
    match fs::create_dir(p) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", p.display())),
    }
    chown(p, uid, gid, mode)
}

fn chown(p: &Path, uid: u32, gid: u32, mode: u32) -> Result<(), String> {
    std::os::unix::fs::chown(p, Some(uid), Some(gid))
        .map_err(|e| format!("chown {}: {e}", p.display()))?;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", p.display()))
}

fn open_dir(p: &Path) -> Result<OwnedFd, String> {
    let fd = unsafe {
        libc::open(
            cstr(p).as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return last_err(p.display());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` relative to `dir`.
fn openat2(dir: &OwnedFd, rel: &str, flags: i32) -> Result<OwnedFd, String> {
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (flags | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
    let path = std::ffi::CString::new(rel).unwrap();
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir.as_raw_fd(),
            path.as_ptr(),
            &how as *const libc::open_how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        return last_err(rel);
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

fn fstat(fd: &OwnedFd) -> Result<libc::stat, String> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return last_err("fstat");
    }
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline_format() {
        assert_eq!(
            cmdline(
                Ipv4Addr::new(172, 31, 0, 2),
                Ipv4Addr::new(172, 31, 0, 1),
                "kali-base"
            ),
            "console=ttyS0 reboot=k panic=1 ip=172.31.0.2::172.31.0.1:255.255.255.252:kali-base:eth0:off"
        );
    }

    #[test]
    fn names_and_tags() {
        assert!(valid_name("kali-base") && valid_name("a"));
        assert!(
            !valid_name("Kali")
                && !valid_name("1abc")
                && !valid_name("")
                && !valid_name(&"a".repeat(32))
        );
        assert!(valid_tag("2026-11-02") && valid_tag("v1.0_rc"));
        assert!(
            !valid_tag(".hidden")
                && !valid_tag("a/b")
                && !valid_tag("..")
                && !valid_tag(&"a".repeat(65))
        );
    }
}
