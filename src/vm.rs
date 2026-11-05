//! `run`, `stop`, `ps`, `console`, `logs`, `shell`, `exec`, `cp` and the internal `launch`
//! (docs/TECHNICAL.md, "Run e boot", "Acesso", "Stop").

use crate::home;
use crate::image::image_ref;
use crate::manifest::{self, parse_size};
use crate::supervise::{self, valid_name};
use crate::util;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RUN: &str = "/run/ohmlatl";
const DETACH: u8 = 0x1d; // Ctrl-]

fn say(msg: impl std::fmt::Display) {
    println!("[ohmlatl] {msg}");
}

fn vmdir(uid: u32, name: &str) -> PathBuf {
    Path::new(RUN).join(uid.to_string()).join(name)
}

fn unit(uid: u32, name: &str) -> String {
    format!("ohmlatl-{uid}-{name}")
}

/// One command on the supervisor's `ctl.sock`.
fn ctl(name: &str, cmd: &str) -> Result<String, String> {
    let p = vmdir(util::uid(), name).join("ctl.sock");
    let mut s = UnixStream::connect(&p).map_err(|_| format!("no VM {name} running"))?;
    let err = |e: io::Error| format!("{}: {e}", p.display());
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(err)?;
    s.write_all(format!("{cmd}\n").as_bytes()).map_err(err)?;
    s.shutdown(std::net::Shutdown::Write).map_err(err)?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(err)?;
    match out.strip_prefix("error: ") {
        Some(e) => Err(format!("{name}: {}", e.trim())),
        None => Ok(out),
    }
}

/// `status` (or `ready`) as key/value pairs: image, user, tap, gateway, guest, pid, started.
fn status(name: &str, cmd: &str) -> Result<HashMap<String, String>, String> {
    Ok(ctl(name, cmd)?
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

fn get<'a>(m: &'a HashMap<String, String>, k: &str) -> &'a str {
    m.get(k).map_or("-", |v| v)
}

/// `systemctl show -p ActiveState,Result <unit>`.
fn unit_state(unit: &str) -> (String, String) {
    let out = Command::new("systemctl")
        .args(["show", "-p", "ActiveState,Result", unit])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let val = |k: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(k))
            .unwrap_or("")
            .to_string()
    };
    (val("ActiveState="), val("Result="))
}

fn check_vcpus(v: &str) -> Result<u32, String> {
    v.parse()
        .ok()
        .filter(|n| (1..=32).contains(n))
        .ok_or(format!("--vcpus {v}: want 1 to 32"))
}

fn check_memory(m: &str) -> Result<(), String> {
    match parse_size(m) {
        Some(b) if b >= 512 << 20 => Ok(()),
        _ => Err(format!("--memory {m}: want a size of at least 512MiB")),
    }
}

/// `ohmlatl run <name>:<tag> [--vcpus n] [--memory n]`.
pub fn run(
    home_flag: Option<&Path>,
    r: &str,
    vcpus: Option<String>,
    memory: Option<String>,
) -> Result<(), String> {
    let (name, tag) = image_ref(r)?;
    let home = home::discover(home_flag)?;
    home::check_owner(&home)?;
    let dir = home.join("images").join(name).join(tag);
    if !dir.join("rootfs.luks").is_file() {
        return Err(format!("no image {r}"));
    }
    let text = fs::read_to_string(dir.join("manifest.yaml"))
        .map_err(|e| format!("{}: {e}", dir.join("manifest.yaml").display()))?;
    let m = manifest::parse(&text).map_err(|e| format!("manifest.yaml: {e}"))?;
    if !m.forward.is_empty() {
        return Err(supervise::FORWARD_UNSUPPORTED.into());
    }
    if let Some(v) = &vcpus {
        check_vcpus(v)?;
    }
    if let Some(mem) = &memory {
        check_memory(mem)?;
    }
    if ctl(name, "status").is_ok() {
        return Err(format!("{name} is already running (ohmlatl stop {name})"));
    }
    let (uid, gid) = (util::uid(), util::gid());
    // A supervisor that just died leaves the unit in ExecStopPost for a moment; systemd-run
    // would refuse the name until it is gone.
    while unit_state(&unit(uid, name)).0 == "deactivating" {
        std::thread::sleep(Duration::from_millis(500));
    }
    let exe = fs::read_link("/proc/self/exe").map_err(|e| format!("/proc/self/exe: {e}"))?;
    home::check_not_foreign_writable(&exe)?;
    home::check_not_foreign_writable(&home)?;

    say(format!("vm {name}"));
    say(format!("rootfs: images/{name}/{tag}/rootfs.luks (LUKS2)"));
    say(format!(
        "internet: {}",
        if m.internet { "on" } else { "off" }
    ));
    say("forwards: none");
    let _ = fs::remove_file(vmdir(uid, name).join("error"));
    say("starting firecracker (jailer)");

    let mut cmd = Command::new("sudo");
    cmd.arg(&exe)
        .arg("--home")
        .arg(&home)
        .args([
            "launch",
            "--uid",
            &uid.to_string(),
            "--gid",
            &gid.to_string(),
        ])
        .args(["--image", r]);
    if let Some(v) = &vcpus {
        cmd.args(["--vcpus", v]);
    }
    if let Some(mem) = &memory {
        cmd.args(["--memory", mem]);
    }
    // sudo clears the environment.
    if let Some(p) = std::env::var_os("OHMLATL_POOL").filter(|p| !p.is_empty()) {
        cmd.arg("--pool").arg(p);
    }
    util::run(&mut cmd, None)?;

    let s = wait_ready(name)?;
    say(format!(
        "network: {} {}/30, guest {}",
        get(&s, "tap"),
        get(&s, "gateway"),
        get(&s, "guest")
    ));
    if unsafe { libc::isatty(0) } != 1 {
        say(format!(
            "stdin is not a terminal: serial not attached; use `ohmlatl console {name}`"
        ));
        return Ok(());
    }
    attach(name)
}

/// Waits for `ready`, watching the unit: `failed`, or `inactive` before `ready`, is a failure.
fn wait_ready(name: &str) -> Result<HashMap<String, String>, String> {
    let (uid, unit) = (util::uid(), unit(util::uid(), name));
    loop {
        if let Ok(s) = status(name, "ready") {
            return Ok(s);
        }
        let (state, _) = unit_state(&unit);
        if state == "failed" || state == "inactive" {
            return Err(match fs::read_to_string(vmdir(uid, name).join("error")) {
                Ok(e) => e.trim().to_string(),
                Err(_) => format!("the supervisor stopped; see: sudo journalctl -u {unit}"),
            });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The terminal in raw mode until dropped.
struct Raw(libc::termios);

impl Raw {
    fn new() -> Result<Raw, String> {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut t) } != 0 {
            return util::last_err("tcgetattr");
        }
        let old = t;
        unsafe {
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
        Ok(Raw(old))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}

/// Copies the console both ways until Ctrl-] or until the VM goes away.
fn attach(name: &str) -> Result<(), String> {
    let p = vmdir(util::uid(), name).join("console.sock");
    let mut sock = UnixStream::connect(&p).map_err(|_| format!("no VM {name} running"))?;
    say("serial attached; Ctrl-] to detach");
    let raw = Raw::new()?;
    let mut out = io::stdout();
    let mut buf = [0u8; 4096];
    let mut first = true;
    let detached = loop {
        let mut fds = [
            libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: sock.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break true;
        }
        if fds[1].revents != 0 {
            let n = sock.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break false;
            }
            if first && buf[..n].starts_with(supervise::BUSY) {
                drop(raw);
                return Err("console busy: another client is attached".into());
            }
            first = false;
            let _ = out.write_all(&buf[..n]).and_then(|()| out.flush());
        }
        if fds[0].revents != 0 {
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break true;
            }
            let data = &buf[..n as usize];
            let end = data.iter().position(|b| *b == DETACH);
            if sock.write_all(&data[..end.unwrap_or(data.len())]).is_err() {
                break false;
            }
            if end.is_some() {
                break true;
            }
        }
    };
    drop(raw);
    println!();
    say(if detached {
        "detached"
    } else {
        "console closed"
    });
    Ok(())
}

/// `ohmlatl console <vm>`.
pub fn console(name: &str) -> Result<(), String> {
    if unsafe { libc::isatty(0) } != 1 {
        return Err("console needs a terminal".into());
    }
    ctl(name, "status")?;
    attach(name)
}

/// `ohmlatl stop <vm>`: returns once the unit is `inactive` or `failed`, after `ExecStopPost`.
pub fn stop(name: &str) -> Result<(), String> {
    ctl(name, "stop")?;
    say(format!("stopping {name}"));
    let unit = unit(util::uid(), name);
    loop {
        let (state, _) = unit_state(&unit);
        if state == "inactive" || state == "failed" {
            say("stopped");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `ohmlatl ps`: the VMs whose `ctl.sock` answers.
pub fn ps() -> Result<(), String> {
    let mut names: Vec<String> = fs::read_dir(Path::new(RUN).join(util::uid().to_string()))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| valid_name(n))
        .collect();
    names.sort();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut rows = vec![["NAME", "IMAGE", "PID", "GUEST", "UPTIME"].map(String::from)];
    for n in names {
        let Ok(s) = status(&n, "status") else {
            continue;
        };
        let up = get(&s, "started")
            .parse::<u64>()
            .map_or("-".into(), |t| uptime(now.saturating_sub(t)));
        rows.push([
            n,
            get(&s, "image").into(),
            get(&s, "pid").into(),
            get(&s, "guest").into(),
            up,
        ]);
    }
    let w: Vec<usize> = (0..4)
        .map(|i| rows.iter().map(|r| r[i].len()).max().unwrap())
        .collect();
    for r in &rows {
        println!(
            "{:<a$}   {:<b$}   {:<c$}   {:<d$}   {}",
            r[0],
            r[1],
            r[2],
            r[3],
            r[4],
            a = w[0],
            b = w[1],
            c = w[2],
            d = w[3]
        );
    }
    Ok(())
}

fn uptime(s: u64) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// `ohmlatl logs <vm> [-f]`: the serial log, kept until the next `run`.
pub fn logs(name: &str, follow: bool) -> Result<(), String> {
    let p = vmdir(util::uid(), name).join("serial.log");
    let mut f = File::open(&p).map_err(|_| format!("no logs for {name}"))?;
    let mut out = io::stdout();
    let err = |e: io::Error| format!("{}: {e}", p.display());
    io::copy(&mut f, &mut out).map_err(err)?;
    if !follow {
        return Ok(());
    }
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let len = f.metadata().map_err(err)?.len();
        // shortcut: after the 1 MiB rotation, -f prints the kept newest half again; track
        // the last bytes printed if that ever matters.
        if len < f.stream_position().map_err(err)? {
            f.seek(SeekFrom::Start(0)).map_err(err)?;
        }
        io::copy(&mut f, &mut out).map_err(err)?;
        out.flush().map_err(err)?;
    }
}

/// The SSH options of docs/TECHNICAL.md, "Acesso", without `ControlMaster`.
fn ssh_opts(sock: &Path) -> Vec<String> {
    [
        "-F",
        "/dev/null",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "GlobalKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
        "-o",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([format!("ControlPath={}", sock.display())])
    .collect()
}

/// The master connection of this boot, opened (asking for the password) if missing. Returns
/// `<user>@<guest>` and the options for the calls that reuse it.
fn master(name: &str) -> Result<(String, Vec<String>), String> {
    let s = status(name, "status")?;
    let dest = format!("{}@{}", get(&s, "user"), get(&s, "guest"));
    let sock = vmdir(util::uid(), name).join("ssh.sock");
    let mut opts = ssh_opts(&sock);
    let alive = Command::new("ssh")
        .args(&opts)
        .args(["-O", "check", "ohmlatl"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !alive {
        // A socket left by a dead master would make ssh skip multiplexing.
        let _ = fs::remove_file(&sock);
        let ok = Command::new("ssh")
            .args(&opts)
            .args([
                "-o",
                "ControlMaster=yes",
                "-o",
                "ControlPersist=yes",
                "-fN",
                &dest,
            ])
            .status()
            .map_err(|e| format!("ssh: {e}"))?
            .success();
        if !ok {
            return Err(format!("ssh login to {dest} failed"));
        }
    }
    opts.extend(["-o".into(), "ControlMaster=no".into()]);
    Ok((dest, opts))
}

/// `ohmlatl shell <vm> [-- cmd]`, and `ohmlatl exec <vm> -- cmd` with `tty` false.
pub fn ssh(name: &str, cmd: &[String], tty: bool) -> Result<(), String> {
    let (dest, opts) = master(name)?;
    let mut c = Command::new("ssh");
    c.args(&opts);
    if tty && !cmd.is_empty() {
        c.arg("-t");
    }
    c.arg(&dest).args(cmd);
    Err(format!("ssh: {}", c.exec()))
}

/// `ohmlatl cp <vm>:<path> <local>` and `ohmlatl cp <local> <vm>:<path>`.
pub fn cp(src: &str, dst: &str) -> Result<(), String> {
    fn remote(s: &str) -> Option<(&str, &str)> {
        s.split_once(':').filter(|(n, _)| valid_name(n))
    }
    let name = match (remote(src), remote(dst)) {
        (Some((n, _)), None) | (None, Some((n, _))) => n,
        _ => return Err("cp needs exactly one <vm>:<path>".into()),
    };
    let (dest, opts) = master(name)?;
    let arg = |s: &str| match remote(s) {
        Some((_, p)) => format!("{dest}:{p}"),
        None => s.to_string(),
    };
    let mut c = Command::new("scp");
    c.arg("-r").args(&opts).arg(arg(src)).arg(arg(dst));
    Err(format!("scp: {}", c.exec()))
}

pub struct Launch {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub image: String,
    pub vcpus: Option<String>,
    pub memory: Option<String>,
    pub pool: Option<String>,
}

/// `launch` (root, short): tmpfs, binaries, VM directory, then the supervisor as the system
/// unit `ohmlatl-<uid>-<name>`, tied to the mount unit of the drive.
pub fn launch(a: Launch) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("launch must run as root".into());
    }
    let (name, _) = image_ref(&a.image)?;
    if !a.home.is_absolute() {
        return Err("--home must be absolute".into());
    }
    if let Some(v) = &a.vcpus {
        check_vcpus(v)?;
    }
    if let Some(m) = &a.memory {
        check_memory(m)?;
    }
    let bin = supervise::prepare_run()?;
    let dir = vmdir(a.uid, name);
    for d in [dir.parent().unwrap(), &dir] {
        supervise::mkdir_owned(d, 0o700, a.uid, a.gid)?;
    }
    let exe = bin.join("ohmlatl");
    let unit = unit(a.uid, name);
    let mut props = vec![
        "Delegate=yes".to_string(),
        "KillMode=mixed".into(),
        "TimeoutStopSec=150".into(),
    ];
    if let Some(m) = mount_unit(&a.home)? {
        props.extend(bind_props(&m));
    }
    props.push(format!(
        "ExecStopPost={} cleanup {} {name}",
        exe.display(),
        a.uid
    ));
    let mut args = vec![
        exe.to_string_lossy().into_owned(),
        "--home".into(),
        a.home.to_string_lossy().into_owned(),
        "supervise".into(),
        "--uid".into(),
        a.uid.to_string(),
        "--gid".into(),
        a.gid.to_string(),
        "--image".into(),
        a.image.clone(),
    ];
    for (flag, v) in [
        ("--vcpus", &a.vcpus),
        ("--memory", &a.memory),
        ("--pool", &a.pool),
    ] {
        if let Some(v) = v {
            args.extend([flag.to_string(), v.clone()]);
        }
    }
    let _ = Command::new("systemctl")
        .args(["reset-failed", &unit])
        .stderr(Stdio::null())
        .status();
    let mut cmd = Command::new("systemd-run");
    cmd.args(["--quiet", "--unit", &unit]);
    for p in &props {
        cmd.args(["-p", p]);
    }
    cmd.arg("--").args(args.iter().map(|a| dollars(a)));
    util::run(&mut cmd, None)
}

/// The mount unit of the filesystem that holds `home`, or `None` on `/`.
fn mount_unit(home: &Path) -> Result<Option<String>, String> {
    let out = |c: &mut Command| -> Result<String, String> {
        let o = c.output().map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
        }
        let s = String::from_utf8_lossy(&o.stdout);
        Ok(s.strip_suffix('\n').unwrap_or(&s).to_string())
    };
    let target = out(Command::new("findmnt")
        .args(["-n", "-o", "TARGET", "-T"])
        .arg(home))
    .map_err(|e| format!("findmnt {}: {e}", home.display()))?;
    if target == "/" {
        return Ok(None);
    }
    out(Command::new("systemd-escape").args(["--path", "--suffix=mount", &target]))
        .map(Some)
        .map_err(|e| format!("systemd-escape {target}: {e}"))
}

/// `BindsTo=` and `After=` for `systemd-run -p`: quoted, each `\` doubled, because
/// systemd-run unescapes the value once.
fn bind_props(mount_unit: &str) -> [String; 2] {
    let q = format!("\"{}\"", mount_unit.replace('\\', "\\\\"));
    [format!("BindsTo={q}"), format!("After={q}")]
}

/// systemd expands `$VAR` in command arguments; `$$` is a literal `$`.
fn dollars(s: &str) -> String {
    s.replace('$', "$$")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_properties() {
        let m = r"run-media-lucy-USB\x20DISK\x24x.mount";
        assert_eq!(
            bind_props(m),
            [
                r#"BindsTo="run-media-lucy-USB\\x20DISK\\x24x.mount""#.to_string(),
                r#"After="run-media-lucy-USB\\x20DISK\\x24x.mount""#.to_string(),
            ]
        );
        assert_eq!(dollars("/media/u/a$b c/ohmlatl"), "/media/u/a$$b c/ohmlatl");
    }

    #[test]
    fn uptimes() {
        assert_eq!(uptime(59), "59s");
        assert_eq!(uptime(120), "2m");
        assert_eq!(uptime(7200), "2h");
        assert_eq!(uptime(3 * 86400), "3d");
    }
}
