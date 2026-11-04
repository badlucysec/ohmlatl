//! `ohmlatl doctor [--build|--run]` (docs/TECHNICAL.md, "Doctor").

use crate::assets;
use crate::home;
use crate::util::{self, cache_home, cstr, find_tool, free_bytes, gib, version};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Below this, `doctor` fails the free-space checks. The build exports whole systems there.
const MIN_CACHE_FREE: u64 = 5 << 30;
const MIN_DRIVE_FREE: u64 = 1 << 30;

#[derive(Default)]
struct Report {
    failed: bool,
}

impl Report {
    fn ok(&self, msg: impl std::fmt::Display) {
        println!("  [ok] {msg}");
    }
    fn warn(&self, msg: impl std::fmt::Display) {
        println!("  [warn] {msg}");
    }
    fn fail(&mut self, msg: impl std::fmt::Display, fix: impl std::fmt::Display) {
        println!("  [fail] {msg}\n         fix: {fix}");
        self.failed = true;
    }
    fn check(&mut self, r: Result<String, (String, String)>) {
        match r {
            Ok(m) => self.ok(m),
            Err((m, fix)) => self.fail(m, fix),
        }
    }
}

pub fn doctor(home_flag: Option<&Path>, build: bool, run: bool) -> Result<bool, String> {
    let mut r = Report::default();
    println!("DRIVE");
    drive(&mut r, home_flag, build);
    if build {
        println!("\nBUILD");
        build_checks(&mut r);
    }
    if run {
        println!("\nRUN");
        run_checks(&mut r);
    }
    Ok(!r.failed)
}

fn drive(r: &mut Report, home_flag: Option<&Path>, build: bool) {
    let dir = match home::discover(home_flag) {
        Ok(d) => d,
        Err(e) => return r.fail(e, "ohmlatl init <dir>, or --home <dir>"),
    };
    let fs_name = match home::filesystem(&dir) {
        Ok(f) => f,
        Err(e) => {
            return r.fail(
                format!("{}: {e}", dir.display()),
                "put the ohmlatl directory on ext4, btrfs or xfs",
            );
        }
    };
    let exec = if home::noexec(&dir) { "noexec" } else { "exec" };
    match home::check_owner(&dir) {
        Ok(()) => r.ok(format!(
            "{} ({fs_name}, {exec}, uid {})",
            dir.display(),
            util::uid()
        )),
        Err(e) => return r.fail(e, home::chown_fix(&dir)),
    }
    if exec == "noexec" {
        r.warn("mounted noexec: the binary on the drive will not run; remount without noexec or use a host ohmlatl with --home");
    }
    match free_bytes(&dir) {
        Ok(n) if n < MIN_DRIVE_FREE => r.fail(
            format!("{} free on the drive", gib(n)),
            "free space on the drive",
        ),
        Ok(n) => r.ok(format!("{} free on the drive", gib(n))),
        Err(e) => r.fail(format!("{}: {e}", dir.display()), "check the drive"),
    }
    leftovers(r, &dir, build);
}

/// `.partial` directories of dead builds (lock free) and `ohmlatl-build-*` mappers.
/// `doctor --build` cleans them under the build lock, like the next build would.
fn leftovers(r: &mut Report, dir: &Path, clean: bool) {
    let mappers = build_mappers();
    let mut found = false;
    for name in fs::read_dir(dir.join("images"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for e in fs::read_dir(name.path()).into_iter().flatten().flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            let Some(tag) = f.strip_prefix('.').and_then(|f| f.strip_suffix(".partial")) else {
                continue;
            };
            let lock_path = name.path().join(format!(".{tag}.lock"));
            let Ok(lock) = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
            else {
                continue;
            };
            if unsafe {
                libc::flock(
                    std::os::fd::AsRawFd::as_raw_fd(&lock),
                    libc::LOCK_EX | libc::LOCK_NB,
                )
            } != 0
            {
                continue; // a build is running
            }
            found = true;
            let part = e.path();
            if !clean {
                r.fail(
                    format!("leftover build {}", part.display()),
                    "ohmlatl doctor --build",
                );
                continue;
            }
            match clean_partial(&part, &mappers) {
                Ok(()) => r.ok(format!("removed leftover build {}", part.display())),
                Err(e) => r.fail(
                    format!("{}: {e}", part.display()),
                    format!("rm -rf {}", part.display()),
                ),
            }
        }
    }
    for (m, backing) in build_mappers() {
        found = true;
        r.fail(
            format!("mapper {m} open (on {})", backing.display()),
            format!("sudo cryptsetup close {m}"),
        );
    }
    if !found {
        r.ok("no leftover builds");
    }
}

/// Removes the `.partial` of a dead build, closing its mappers first. Call with its lock held.
pub fn clean_partial(part: &Path, mappers: &[(String, PathBuf)]) -> std::io::Result<()> {
    for (m, backing) in mappers {
        if backing.starts_with(part) {
            let _ = Command::new("sudo")
                .args(["cryptsetup", "close", m])
                .status();
        }
    }
    fs::remove_dir_all(part)
}

/// `ohmlatl-build-*` device-mapper names with the file behind their loop device.
pub fn build_mappers() -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    for e in fs::read_dir("/sys/block").into_iter().flatten().flatten() {
        let p = e.path();
        let Ok(name) = fs::read_to_string(p.join("dm/name")) else {
            continue;
        };
        let name = name.trim().to_string();
        if !name.starts_with("ohmlatl-build-") {
            continue;
        }
        let backing = fs::read_dir(p.join("slaves"))
            .into_iter()
            .flatten()
            .flatten()
            .find_map(|s| fs::read_to_string(s.path().join("loop/backing_file")).ok())
            .map(|b| PathBuf::from(b.trim()))
            .unwrap_or_default();
        out.push((name, backing));
    }
    out
}

fn tool_version(
    r: &mut Report,
    name: &str,
    args: &[&str],
    min: (u32, u32),
    label: &str,
    warn_only: bool,
) {
    let found = find_tool(name).and_then(|t| {
        let o = Command::new(&t).args(args).output().ok()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        version(&text)
    });
    let want = format!("{label} >= {}.{}", min.0, min.1);
    match found {
        Some(v) if v >= min => r.ok(format!("{label} {}.{}", v.0, v.1)),
        Some(v) if warn_only => r.warn(format!(
            "{label} {}.{} (want {want}, for image resize and image passwd)",
            v.0, v.1
        )),
        None if warn_only => r.warn(format!(
            "{label} not found (needed by image resize and image passwd)"
        )),
        Some(v) => r.fail(
            format!("{label} {}.{}", v.0, v.1),
            format!("install {want}"),
        ),
        None => r.fail(format!("{label} not found"), format!("install {want}")),
    }
}

fn present(r: &mut Report, name: &str, fix: &str) {
    match find_tool(name) {
        Some(p) => r.ok(format!("{name} ({})", p.display())),
        None => r.fail(format!("{name} not found"), fix),
    }
}

fn build_checks(r: &mut Report) {
    r.check(podman_rootless());
    r.check(subids());
    for t in ["newuidmap", "newgidmap"] {
        r.check(setid_helper(t));
    }
    r.check(
        char_dev(Path::new("/dev/fuse"), true)
            .map_err(|e| (e, "load the fuse module (modprobe fuse)".into())),
    );
    apparmor(r);
    tool_version(r, "mkfs.ext4", &["-V"], (1, 45), "e2fsprogs", false);
    tool_version(r, "cryptsetup", &["--version"], (2, 1), "cryptsetup", false);
    r.check(ssh_keygen());
    present(r, "udevadm", "install udev");
    present(r, "sudo", "install sudo");
    match cache_home().and_then(|c| free_bytes(&c).map(|n| (c, n))) {
        Ok((c, n)) if n < MIN_CACHE_FREE => r.fail(
            format!("{} free in {}", gib(n), c.display()),
            format!("free at least {}", gib(MIN_CACHE_FREE)),
        ),
        Ok((c, n)) => r.ok(format!("{} free in {}", gib(n), c.display())),
        Err(e) => r.fail(
            format!("cache directory: {e}"),
            "set HOME or XDG_CACHE_HOME",
        ),
    }
}

fn podman_rootless() -> Result<String, (String, String)> {
    let fix = "see the Podman rootless requirements in docs/TECHNICAL.md (subuid/subgid, newuidmap, /dev/fuse)";
    let mut cmd =
        assets::podman().map_err(|e| (format!("embedded podman: {e}"), fix.to_string()))?;
    let o = cmd
        .args([
            "info",
            "--format",
            "{{.Host.Security.Rootless}} {{.Store.GraphDriverName}}",
        ])
        .output()
        .map_err(|e| (format!("embedded podman: {e}"), fix.to_string()))?;
    let out = String::from_utf8_lossy(&o.stdout);
    if o.status.success() && out.starts_with("true") {
        Ok(format!(
            "podman rootless ({})",
            out.trim().trim_start_matches("true ")
        ))
    } else {
        let err = String::from_utf8_lossy(&o.stderr);
        Err((
            format!(
                "podman rootless: {}",
                err.lines().last().unwrap_or("failed")
            ),
            fix.to_string(),
        ))
    }
}

fn subids() -> Result<String, (String, String)> {
    let uid = util::uid();
    let name = util::user_name(uid).unwrap_or_default();
    let count = |file: &str| -> u64 {
        fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.len() == 3 && (f[0] == name || f[0] == uid.to_string()))
                    .then(|| f[2].parse::<u64>().ok())?
            })
            .sum()
    };
    let (u, g) = (count("/etc/subuid"), count("/etc/subgid"));
    if u >= 65536 && g >= 65536 {
        Ok(format!("subuid/subgid ({} ids)", u.min(g)))
    } else {
        Err((
            format!("subuid/subgid: {u}/{g} ids for {name}, need 65536"),
            format!("sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 {name}"),
        ))
    }
}

fn setid_helper(name: &str) -> Result<String, (String, String)> {
    let fix = "install the uidmap package (shadow-utils on Fedora)".to_string();
    let p = find_tool(name).ok_or((format!("{name} not found"), fix.clone()))?;
    let m = fs::metadata(&p).map_err(|e| (format!("{}: {e}", p.display()), fix.clone()))?;
    if m.uid() == 0 && m.mode() & 0o4000 != 0 {
        return Ok(format!("{name} (setuid)"));
    }
    let attr = c"security.capability";
    let n = unsafe { libc::getxattr(cstr(&p).as_ptr(), attr.as_ptr(), std::ptr::null_mut(), 0) };
    if n > 0 {
        return Ok(format!("{name} (file caps)"));
    }
    Err((
        format!("{name} is neither setuid root nor has file capabilities"),
        fix,
    ))
}

fn char_dev(p: &Path, rw: bool) -> Result<String, String> {
    let m = fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !m.file_type().is_char_device() {
        return Err(format!("{} is not a character device", p.display()));
    }
    if rw && unsafe { libc::access(cstr(p).as_ptr(), libc::R_OK | libc::W_OK) } != 0 {
        return Err(format!("{} is not readable and writable", p.display()));
    }
    Ok(p.display().to_string())
}

fn apparmor(r: &mut Report) {
    let restricted = fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .is_ok_and(|s| s.trim() == "1");
    if !restricted {
        return;
    }
    let loaded = fs::read_to_string("/sys/kernel/security/apparmor/profiles")
        .is_ok_and(|p| p.lines().any(|l| l.starts_with("ohmlatl-podman ")));
    if loaded {
        return r.ok("AppArmor profile ohmlatl-podman");
    }
    let cache = cache_home().unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    let glob = if cache == Path::new(&home).join(".cache") && home.starts_with("/home/") {
        "/home/*/.cache/ohmlatl/bin/*/bin/podman".to_string()
    } else {
        format!("{}/ohmlatl/bin/*/bin/podman", cache.display())
    };
    r.fail(
        "AppArmor restricts user namespaces (kernel.apparmor_restrict_unprivileged_userns=1)",
        format!(
            "install this profile once:\n\n\
             # /etc/apparmor.d/ohmlatl-podman\n\
             abi <abi/4.0>,\n\
             include <tunables/global>\n\
             profile ohmlatl-podman {glob} flags=(unconfined) {{\n  userns,\n  include if exists <local/ohmlatl-podman>\n}}\n\n\
             sudo apparmor_parser -r /etc/apparmor.d/ohmlatl-podman"
        ),
    );
}

fn ssh_keygen() -> Result<String, (String, String)> {
    let fix = "install an OpenSSH ssh-keygen that supports -A -f".to_string();
    let t = find_tool("ssh-keygen").ok_or(("ssh-keygen not found".to_string(), fix.clone()))?;
    let dir = std::env::temp_dir().join(format!("ohmlatl-doctor-{}", std::process::id()));
    let _ = fs::create_dir_all(dir.join("etc/ssh"));
    let ok = Command::new(&t)
        .arg("-A")
        .arg("-f")
        .arg(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let keys = fs::read_dir(dir.join("etc/ssh"))
        .map(|d| d.count())
        .unwrap_or(0);
    let _ = fs::remove_dir_all(&dir);
    match ok {
        Ok(s) if s.success() && keys > 0 => Ok("ssh-keygen -A -f".into()),
        _ => Err(("ssh-keygen -A -f does not work".into(), fix)),
    }
}

fn run_checks(r: &mut Report) {
    // The jailer creates its own /dev/kvm node for the operator's UID, so only the device matters.
    r.check(
        char_dev(Path::new("/dev/kvm"), false)
            .map(|m| format!("{m} present"))
            .map_err(|e| {
                (
                    e,
                    "enable virtualization in the firmware and load kvm_intel or kvm_amd".into(),
                )
            }),
    );
    r.check(
        char_dev(Path::new("/dev/net/tun"), false)
            .map_err(|e| (e, "load the tun module (modprobe tun)".into())),
    );
    present(r, "ip", "install iproute2");
    r.check(nft_inet_nat());
    let cg = util::statfs(Path::new("/sys/fs/cgroup")).map(|s| s.f_type);
    match cg {
        Ok(0x6367_7270) => r.ok("cgroup v2"),
        _ => r.fail(
            "/sys/fs/cgroup is not cgroup v2",
            "boot with systemd.unified_cgroup_hierarchy=1",
        ),
    }
    present(r, "systemd-run", "the host needs systemd");
    present(r, "sudo", "install sudo");
    present(r, "ssh", "install the OpenSSH client");
    firewall(r);
    tool_version(r, "cryptsetup", &["--version"], (2, 1), "cryptsetup", true);
    tool_version(r, "mkfs.ext4", &["-V"], (1, 45), "e2fsprogs", true);
}

/// nat in the `inet` family, tried in a throwaway user and network namespace.
fn nft_inet_nat() -> Result<String, (String, String)> {
    let fix = "install nftables; the kernel needs nf_tables with inet nat (>= 5.2)".to_string();
    let nft = find_tool("nft").ok_or(("nft not found".to_string(), fix.clone()))?;
    let unshare = find_tool("unshare").ok_or((
        "unshare not found".to_string(),
        "install util-linux".to_string(),
    ))?;
    let mut child = Command::new(unshare)
        .arg("-rn")
        .arg(&nft)
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| (format!("unshare: {e}"), fix.clone()))?;
    let _ = child.stdin.take().unwrap().write_all(
        b"table inet t {\n chain p {\n  type nat hook postrouting priority srcnat;\n  masquerade\n }\n}\n",
    );
    let o = child
        .wait_with_output()
        .map_err(|e| (e.to_string(), fix.clone()))?;
    if o.status.success() {
        Ok("iproute2 + nftables (inet nat)".into())
    } else {
        Err((
            format!(
                "nft inet nat: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            fix,
        ))
    }
}

/// Drop or reject in INPUT or FORWARD of another table wins over our accept.
fn firewall(r: &mut Report) {
    let o = Command::new("sudo")
        .args(["nft", "list", "ruleset"])
        .output();
    let text = match o {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => {
            return r.fail(
                "cannot read the nft ruleset",
                "run `sudo nft list ruleset` by hand",
            );
        }
    };
    let found = foreign_drops(&text);
    if found.iter().any(|(t, _, _)| t == "inet ohmlatl-fwguard") {
        r.warn("inet ohmlatl-fwguard is active (Docker or libvirt started later will not route); remove: sudo nft delete table inet ohmlatl-fwguard");
    }
    let foreign: Vec<_> = found
        .into_iter()
        .filter(|(t, _, _)| !t.starts_with("inet ohmlatl"))
        .collect();
    if foreign.is_empty() {
        return r.ok("no foreign firewall drop");
    }
    for (table, chain, hook) in foreign {
        // ip/ip6 tables usually belong to iptables-nft (Docker, ufw): fix them with iptables.
        let ipt = match table.split_once(' ') {
            Some(("ip", _)) => Some("iptables"),
            Some(("ip6", _)) => Some("ip6tables"),
            _ => None,
        };
        let rule = |dir: &str| match ipt {
            Some(t) => format!("sudo {t} -I {chain} -{} ohm+ -j ACCEPT", &dir[..1]),
            None => format!("sudo nft insert rule {table} {chain} {dir}name \"ohm*\" accept"),
        };
        let mut fix = rule("iif");
        if hook == "forward" {
            fix += &format!("; {}", rule("oif"));
        }
        r.fail(
            format!("{table} chain {chain} ({hook}) drops or rejects"),
            fix,
        );
    }
}

/// (table, chain, hook) of base chains on input/forward with a drop policy or a drop/reject rule.
fn foreign_drops(ruleset: &str) -> Vec<(String, String, String)> {
    let mut out = vec![];
    let (mut table, mut chain, mut hook) = (String::new(), String::new(), None::<String>);
    let mut flagged = false;
    for line in ruleset.lines().map(str::trim) {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["table", fam, name, ..] => table = format!("{fam} {name}"),
            ["chain", name, ..] => (chain, hook, flagged) = (name.to_string(), None, false),
            ["type", _, "hook", h, ..] => {
                hook = Some(h.to_string());
                if line.contains("policy drop") && matches!(*h, "input" | "forward") {
                    out.push((table.clone(), chain.clone(), h.to_string()));
                    flagged = true;
                }
            }
            _ => {
                if let Some(h) = hook
                    .as_deref()
                    .filter(|h| matches!(*h, "input" | "forward"))
                    && !flagged
                    && w.iter().any(|t| *t == "drop" || *t == "reject")
                {
                    out.push((table.clone(), chain.clone(), h.to_string()));
                    flagged = true;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn foreign_drops() {
        let rs = "table ip filter {\n\tchain INPUT {\n\t\ttype filter hook input priority filter; policy drop;\n\t}\n\
                  \tchain DOCKER {\n\t\tdrop\n\t}\n}\n\
                  table inet firewalld {\n\tchain filter_FORWARD {\n\t\ttype filter hook forward priority filter + 10; policy accept;\n\
                  \t\tct state established accept\n\t\treject with icmpx admin-prohibited\n\t}\n}\n\
                  table inet ok {\n\tchain input {\n\t\ttype filter hook input priority 0; policy accept;\n\t}\n}\n";
        let got = super::foreign_drops(rs);
        assert_eq!(
            got,
            vec![
                ("ip filter".into(), "INPUT".into(), "input".into()),
                (
                    "inet firewalld".into(),
                    "filter_FORWARD".into(),
                    "forward".into()
                )
            ]
        );
    }
}
