//! The VM supervisor and its cleanup (docs/TECHNICAL.md, "Run e boot", "Stop").
//!
//! Runs as root in the system unit `ohmlatl-<uid>-<name>` started by `launch`. Its output goes
//! to the journal through `log`, which never panics: the cleanup must not depend on stderr.

use crate::assets;
use crate::manifest::{self, parse_size};
use crate::net;
use crate::util::{cstr, last_err, log};
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
/// Sent to a second console client before closing it.
pub const BUSY: &[u8] = b"[ohmlatl] console busy: another client is attached\r\n";

pub struct Args {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub name: String,
    pub tag: String,
    /// `run --vcpus` and `--memory`, over the manifest for this boot.
    pub vcpus: Option<u32>,
    pub memory: Option<String>,
    pub pool: String,
}

/// What `boot` needs once the manifest is read and the network is up.
struct Vm {
    vcpus: u32,
    memory_mib: u64,
    user: String,
    tap: String,
    gw: Ipv4Addr,
    guest: Ipv4Addr,
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
        return Err("supervise must run as root".into());
    }
    if !valid_name(&a.name) || !valid_tag(&a.tag) {
        return Err(format!("invalid image {}:{}", a.name, a.tag));
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
    // `launch` runs the copy in /run/ohmlatl/bin/<sha256>/, next to Firecracker and the kernel.
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let bin = exe.parent().unwrap().to_path_buf();
    let vmdir = Path::new(RUN).join(a.uid.to_string()).join(&a.name);
    for d in [vmdir.parent().unwrap(), &vmdir] {
        mkdir_owned(d, 0o700, a.uid, a.gid)?;
    }

    cleanup(a.uid, &a.name);
    // The image lock lives until the cleanup has killed Firecracker.
    let mut held = None;
    let r = supervise(&a, &bin, &vmdir, &mut held);
    cleanup(a.uid, &a.name);
    drop(held);
    if let Err(e) = &r {
        log(format!("supervise: {e}"));
        write_error(&vmdir, a.uid, a.gid, e);
    }
    r
}

/// `error` in the VM directory, for the CLI to print.
fn write_error(vmdir: &Path, uid: u32, gid: u32, msg: &str) {
    let p = vmdir.join("error");
    let _ = fs::remove_file(&p);
    let r = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&p)
        .and_then(|mut f| {
            writeln!(f, "{msg}")?;
            std::os::unix::fs::fchown(&f, Some(uid), Some(gid))
        });
    if let Err(e) = r {
        log(format!("supervise: {}: {e}", p.display()));
    }
}

/// Steps 1 to 7 of the supervisor, then the event loop until Firecracker exits.
fn supervise(a: &Args, bin: &Path, vmdir: &Path, held: &mut Option<OwnedFd>) -> Result<(), String> {
    // The drive is not trusted by root: resolve everything below --home without symlinks.
    let home = open_dir(&a.home)?;
    let image = held.insert(openat2(
        &home,
        &format!("images/{}/{}", a.name, a.tag),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?);
    if unsafe { libc::flock(image.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!("image {}:{} is in use", a.name, a.tag));
    }
    let mf = File::from(owned_file(image, "manifest.yaml", libc::O_RDONLY, a.uid)?);
    let mut text = String::new();
    mf.take(1 << 20)
        .read_to_string(&mut text)
        .map_err(|e| format!("manifest.yaml: {e}"))?;
    let m = manifest::parse(&text).map_err(|e| format!("manifest.yaml: {e}"))?;
    if !m.forward.is_empty() {
        return Err(FORWARD_UNSUPPORTED.into());
    }
    let vcpus = a.vcpus.unwrap_or(m.vcpus);
    if !(1..=32).contains(&vcpus) {
        return Err(format!("--vcpus {vcpus}: want 1 to 32"));
    }
    let memory = a.memory.as_deref().unwrap_or(&m.memory);
    let memory_mib = match parse_size(memory) {
        Some(b) if b >= 512 << 20 => b >> 20,
        _ => return Err(format!("memory {memory}: want a size of at least 512MiB")),
    };
    let rootfs = owned_file(image, "rootfs.luks", libc::O_RDWR, a.uid)?;

    let id = format!("{}-{}", a.uid, a.name);
    let net = net::setup(&id, a.uid, &a.pool, m.internet)?;
    if let Some((udp, tcp)) = net.dns {
        net::serve_dns(udp, tcp, net.guest);
    }
    net::watch_routes();
    let vm = Vm {
        vcpus,
        memory_mib,
        user: m.user.name,
        tap: net.tap,
        gw: net.gw,
        guest: net.guest,
    };
    let root = Path::new(RUN)
        .join("jail/firecracker")
        .join(&id)
        .join("root");
    boot(a, &vm, bin, &root, vmdir, &rootfs)
}

pub const FORWARD_UNSUPPORTED: &str =
    "vm.network.forward is not supported yet; remove the forwards from the image's manifest.yaml";

/// `rel` below `dir`, opened with `openat2`: a regular file owned by `uid`.
fn owned_file(dir: &OwnedFd, rel: &str, flags: i32, uid: u32) -> Result<OwnedFd, String> {
    let fd = openat2(dir, rel, flags)?;
    let st = fstat(&fd)?;
    if st.st_mode & libc::S_IFMT != libc::S_IFREG || st.st_uid != uid {
        return Err(format!("{rel} must be a regular file owned by uid {uid}"));
    }
    Ok(fd)
}

fn boot(
    a: &Args,
    vm: &Vm,
    bin: &Path,
    root: &Path,
    vmdir: &Path,
    rootfs: &OwnedFd,
) -> Result<(), String> {
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

    // The unit's cgroup; without --cgroup the jailer moves Firecracker into it.
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
    log(format!("supervise: jailer started (pid {pid})"));

    let (tx, rx) = mpsc::channel::<Event>();
    spawn_waiter(child, tx.clone());
    follow_log(fc_log);

    // Copy the serial before InstanceStart so no early output is lost.
    let log_path = vmdir.join("serial.log");
    let _ = fs::remove_file(&log_path);
    let serial_log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    std::os::unix::fs::fchown(&serial_log, Some(a.uid), Some(a.gid)).map_err(|e| e.to_string())?;
    let serial = Arc::new(Mutex::new(Serial {
        ring: VecDeque::new(),
        log: serial_log,
        log_len: 0,
        client: None,
    }));
    let master_w = File::from(master.try_clone().map_err(|e| e.to_string())?);
    pump_pty(File::from(master), serial.clone());

    let api_sock = root.join("run/api.sock");
    if let Err(e) = configure(a, vm, &api_sock) {
        unsafe { libc::kill(pid, libc::SIGKILL) };
        let _ = rx.recv_timeout(Duration::from_secs(5));
        return Err(e);
    }

    let ip = vmdir.join("ip");
    let _ = fs::remove_file(&ip);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o644)
        .open(&ip)
        .and_then(|mut f| {
            writeln!(f, "{}", vm.guest)?;
            std::os::unix::fs::fchown(&f, Some(a.uid), Some(a.gid))
        })
        .map_err(|e| format!("{}: {e}", ip.display()))?;
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let status = format!(
        "image {}:{}\nuser {}\ntap {}\ngateway {}\nguest {}\npid {pid}\nstarted {started}\n",
        a.name, a.tag, vm.user, vm.tap, vm.gw, vm.guest
    );
    let console = listen(&vmdir.join("console.sock"), a.uid, a.gid)?;
    let ctl = listen(&vmdir.join("ctl.sock"), a.uid, a.gid)?;
    serve_console(console, serial, master_w);
    serve_ctl(ctl, tx.clone(), status);
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
    log(format!(
        "supervise: ready: {}:{} on {} (guest {}, gateway {})",
        a.name, a.tag, vm.tap, vm.guest, vm.gw
    ));

    let mut deadline: Option<Instant> = None;
    loop {
        let ev = match deadline {
            None => rx.recv().map_err(|_| "event channel closed".to_string())?,
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(ev) => ev,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    log(format!(
                        "supervise: Firecracker did not exit in {}s, killing it",
                        STOP_TIMEOUT.as_secs()
                    ));
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                    deadline = None;
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            },
        };
        match ev {
            Event::Exited(s) => {
                log(format!(
                    "supervise: Firecracker exited: {}",
                    s.map_or_else(|e| e.to_string(), |s| s.to_string())
                ));
                return Ok(());
            }
            Event::Stop if deadline.is_none() => {
                log("supervise: stop: sending Ctrl-Alt-Del");
                deadline = Some(Instant::now() + STOP_TIMEOUT);
                if let Err(e) = api(
                    &api_sock,
                    "/actions",
                    r#"{"action_type": "SendCtrlAltDel"}"#,
                ) {
                    log(format!(
                        "supervise: SendCtrlAltDel: {e}; killing Firecracker"
                    ));
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
            Event::Stop => {}
        }
    }
}

fn configure(a: &Args, vm: &Vm, sock: &Path) -> Result<(), String> {
    let t0 = Instant::now();
    while !sock.exists() {
        if t0.elapsed() > Duration::from_secs(10) {
            return Err(format!("{} did not appear", sock.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let boot = cmdline(vm.guest, vm.gw, &a.name);
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
        &format!(r#"{{"iface_id": "eth0", "host_dev_name": "{}"}}"#, vm.tap),
    )?;
    api(
        sock,
        "/machine-config",
        &format!(
            r#"{{"vcpu_count": {}, "mem_size_mib": {}}}"#,
            vm.vcpus, vm.memory_mib
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
pub fn prepare_run() -> Result<PathBuf, String> {
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

/// `ohmlatl cleanup <uid> <name>`, idempotent: at the end of the supervisor and again as the
/// unit's `ExecStopPost`, which also runs when the supervisor died.
pub fn cleanup(uid: u32, name: &str) {
    let id = format!("{uid}-{name}");
    kill_unit(&id);
    unmount_jail(&Path::new(RUN).join("jail/firecracker").join(&id));
    net::teardown(&id);
    let vmdir = Path::new(RUN).join(uid.to_string()).join(name);
    let ssh = vmdir.join("ssh.sock");
    if ssh.exists() {
        // The destination is a placeholder that ssh requires.
        let _ = Command::new("ssh")
            .args(["-F", "/dev/null", "-o"])
            .arg(format!("ControlPath={}", ssh.display()))
            .args(["-O", "exit", "ohmlatl"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    for f in ["ctl.sock", "console.sock", "ssh.sock", "ip"] {
        let _ = fs::remove_file(vmdir.join(f));
    }
}

/// Kills what is left in the unit's cgroup (Firecracker, after a dead supervisor) but this
/// process, and waits up to 5 s for it to go. Outside the unit (run by hand), kills nothing.
fn kill_unit(id: &str) {
    let cg = fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let Some(cg) = cg.lines().find_map(|l| l.strip_prefix("0::/")) else {
        return;
    };
    if !cg.ends_with(&format!("/ohmlatl-{id}.service")) {
        return;
    }
    let procs = Path::new("/sys/fs/cgroup").join(cg).join("cgroup.procs");
    let me = std::process::id().to_string();
    for _ in 0..50 {
        let text = fs::read_to_string(&procs).unwrap_or_default();
        let others: Vec<i32> = text
            .lines()
            .filter(|p| *p != me)
            .filter_map(|p| p.parse().ok())
            .collect();
        if others.is_empty() {
            return;
        }
        for p in others {
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    log(format!("cleanup: processes left in {}", procs.display()));
}

/// Unmounts the bind mounts and removes the chroot.
fn unmount_jail(jail: &Path) {
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
        log(format!(
            "cleanup: {} still has mounts; not removing it",
            jail.display()
        ));
    } else if jail.exists()
        && let Err(e) = fs::remove_dir_all(jail)
    {
        log(format!("cleanup: {}: {e}", jail.display()));
    }
}

fn spawn_waiter(mut child: Child, tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        let _ = tx.send(Event::Exited(child.wait()));
    });
}

/// Firecracker's own log stays out of the serial: copy it to the journal.
fn follow_log(path: PathBuf) {
    std::thread::spawn(move || {
        let Ok(mut f) = File::open(&path) else { return };
        let mut buf = String::new();
        loop {
            buf.clear();
            match f.read_to_string(&mut buf) {
                Ok(0) => std::thread::sleep(Duration::from_millis(500)),
                Ok(_) => buf.lines().for_each(|l| log(format!("firecracker: {l}"))),
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
                let _ = c.write_all(BUSY);
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

/// The control socket: `ready` and `status` (the same reply: the socket only exists once the
/// VM is ready) and `stop`.
fn serve_ctl(l: UnixListener, tx: mpsc::Sender<Event>, status: String) {
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut line = String::new();
            let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = (&c).take(64).read_to_string(&mut line);
            let reply = match line.trim() {
                "ready" | "status" => &status,
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

pub fn mkdir_owned(p: &Path, mode: u32, uid: u32, gid: u32) -> Result<(), String> {
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
