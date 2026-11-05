//! The managed network of a VM (docs/TECHNICAL.md, "Rede"): pool, `/30`, TAP, `ip_forward`,
//! the `inet ohmlatl` table and the DNS relay. Runs as root, inside the supervisor and cleanup.

use crate::util::{last_err, log, tool};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

const NET: &str = "/run/ohmlatl/net";
const LOCK: &str = "/run/ohmlatl/net.lock";
const FIXED_BLOCKED: [&str; 5] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "100.64.0.0/10",
];
/// Concurrent DNS queries per VM; more are dropped and the guest retries.
const DNS_INFLIGHT: usize = 64;

const TABLE: &str = r#"table inet ohmlatl {
  set gws { type ipv4_addr; }
  set blocked { type ipv4_addr; flags interval; auto-merge; }
  map vm_iif_raw { type ifname : verdict; }
  map vm_iif_nat { type ifname : verdict; }
  map vm_iif_in { type ifname : verdict; }
  map vm_iif_fwd { type ifname : verdict; }
  map vm_oif_out { type ifname : verdict; }
  chain prerouting_raw { type filter hook prerouting priority raw;
    iifname "ohm*" meta nfproto ipv6 drop
    iifname vmap @vm_iif_raw
  }
  chain prerouting_nat { type nat hook prerouting priority dstnat;
    iifname vmap @vm_iif_nat
  }
  chain input { type filter hook input priority filter;
    ip daddr @gws iifname != "lo" iifname != "ohm*" drop
    iifname vmap @vm_iif_in
    iifname "ohm*" reject
  }
  chain forward { type filter hook forward priority filter;
    oifname "ohm*" ct state established,related accept
    iifname "ohm*" oifname "ohm*" reject
    oifname "ohm*" drop
    iifname "ohm*" ip daddr @blocked reject
    iifname vmap @vm_iif_fwd
    iifname "ohm*" reject
  }
  chain output { type filter hook output priority filter;
    oifname vmap @vm_oif_out
  }
  chain postrouting_nat { type nat hook postrouting priority srcnat;
    iifname "ohm*" oifname != "ohm*" masquerade
  }
}
"#;

const FWGUARD: &str = r#"table inet ohmlatl-fwguard {
  chain forward { type filter hook forward priority filter;
    iifname != "ohm*" oifname != "ohm*" drop
  }
}
"#;

/// Per-VM chains, in the order the cleanup removes them after the map elements.
const CHAINS: [(&str, &str); 5] = [
    ("raw", "vm_iif_raw"),
    ("nat", "vm_iif_nat"),
    ("in", "vm_iif_in"),
    ("fwd", "vm_iif_fwd"),
    ("out", "vm_oif_out"),
];

/// An IPv4 prefix: (address as u32, length).
pub type Cidr = (u32, u8);

pub fn parse_cidr(s: &str) -> Option<Cidr> {
    let (a, l) = s.split_once('/').unwrap_or((s, "32"));
    let a: Ipv4Addr = a.parse().ok()?;
    let l: u8 = l.parse().ok()?;
    (l <= 32).then_some((u32::from(a), l))
}

fn mask(l: u8) -> u32 {
    if l == 0 { 0 } else { u32::MAX << (32 - l) }
}

fn show((a, l): Cidr) -> String {
    format!("{}/{l}", Ipv4Addr::from(a & mask(l)))
}

/// `c` lies entirely inside `pool`.
fn inside(c: Cidr, pool: Cidr) -> bool {
    c.1 >= pool.1 && (c.0 ^ pool.0) & mask(pool.1) == 0
}

/// What the host has now: links, IPv4 addresses and routes (any table), each with its device.
#[derive(Default)]
pub struct Host {
    pub links: Vec<String>,
    pub addrs: Vec<(String, Cidr)>,
    pub routes: Vec<(String, Cidr)>,
}

impl Host {
    fn read() -> Result<Host, String> {
        let links = fs::read_dir("/sys/class/net")
            .map_err(|e| format!("/sys/class/net: {e}"))?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        Ok(Host {
            links,
            addrs: parse_addrs(&ip_out(&["-4", "-o", "addr", "show"])?),
            routes: parse_routes(&ip_out(&["-4", "route", "show", "table", "all"])?),
        })
    }
}

/// `ip -4 -o addr show`: "2: wlan0    inet 192.168.0.105/24 brd ...".
fn parse_addrs(text: &str) -> Vec<(String, Cidr)> {
    text.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let dev = w.get(1)?.split('@').next()?;
            (w.get(2) == Some(&"inet")).then_some(())?;
            Some((dev.to_string(), parse_cidr(w.get(3)?)?))
        })
        .collect()
}

/// `ip -4 route show table all`; default routes are left out.
fn parse_routes(text: &str) -> Vec<(String, Cidr)> {
    const TYPES: [&str; 10] = [
        "unicast",
        "local",
        "broadcast",
        "multicast",
        "throw",
        "unreachable",
        "prohibit",
        "blackhole",
        "nat",
        "anycast",
    ];
    text.lines()
        .filter_map(|l| {
            let mut w: Vec<&str> = l.split_whitespace().collect();
            if w.first().is_some_and(|t| TYPES.contains(t)) {
                w.remove(0);
            }
            let dst = parse_cidr(w.first()?)?;
            let dev = w
                .iter()
                .position(|t| *t == "dev")
                .and_then(|i| w.get(i + 1))
                .map_or("", |d| d);
            Some((dev.to_string(), dst))
        })
        .collect()
}

/// An address or route of the host, not on `ohm*` and not the default route, inside the pool.
pub fn conflict(pool: Cidr, host: &Host) -> Option<String> {
    let ours = |dev: &str| dev.starts_with("ohm");
    if let Some((dev, a)) = host
        .addrs
        .iter()
        .find(|(d, a)| !ours(d) && inside((a.0, 32), pool))
    {
        return Some(format!("address {} on {dev}", Ipv4Addr::from(a.0)));
    }
    host.routes
        .iter()
        .find(|(d, r)| !ours(d) && inside(*r, pool))
        .map(|(dev, r)| {
            format!(
                "route {} via {}",
                show(*r),
                if dev.is_empty() { "-" } else { dev }
            )
        })
}

/// The first free `ohm<N>` and the first free `/30` of the pool, against the registries and
/// what is still on `ohm*` links (leftovers of a supervisor that died before its cleanup).
pub fn allocate(pool: Cidr, host: &Host, regs: &[(String, Cidr)]) -> Result<(u32, Cidr), String> {
    let n = (0..)
        .find(|n| {
            let t = format!("ohm{n}");
            !host.links.contains(&t) && !regs.iter().any(|(r, _)| *r == t)
        })
        .unwrap();
    let base = pool.0 & mask(pool.1);
    let count = 1u64 << (32 - pool.1 as u32 - 2);
    let used = |s: u32| {
        regs.iter().any(|(_, c)| c.0 & !3 == s)
            || host
                .addrs
                .iter()
                .any(|(d, a)| d.starts_with("ohm") && a.0 & !3 == s)
    };
    (0..count)
        .map(|i| base + 4 * i as u32)
        .find(|s| !used(*s))
        .map(|s| (n, (s, 30)))
        .ok_or_else(|| format!("no free /30 left in {}", show(pool)))
}

/// `/run/ohmlatl/net/<uid>-<name>`: "ohm<N> <subnet>/30".
fn parse_reg(text: &str) -> Option<(String, Cidr)> {
    let (tap, subnet) = text.trim().split_once(' ')?;
    let n = tap.strip_prefix("ohm")?;
    (!n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())).then_some(())?;
    Some((tap.to_string(), parse_cidr(subnet).filter(|c| c.1 == 30)?))
}

fn registries() -> Vec<(String, Cidr)> {
    fs::read_dir(NET)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| parse_reg(&fs::read_to_string(e.path()).ok()?))
        .collect()
}

/// `flock /run/ohmlatl/net.lock`, held until the file is dropped.
fn lock() -> Result<File, String> {
    fs::create_dir_all(NET).map_err(|e| format!("{NET}: {e}"))?;
    fs::set_permissions(NET, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(LOCK)
        .map_err(|e| format!("{LOCK}: {e}"))?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return last_err(format!("flock {LOCK}"));
    }
    Ok(f)
}

fn ip_out(args: &[&str]) -> Result<String, String> {
    let o = Command::new(tool("ip")?)
        .args(args)
        .output()
        .map_err(|e| format!("ip: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "ip {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn ip(args: &[&str]) -> Result<(), String> {
    ip_out(args).map(drop)
}

/// `nft -f -` with `script`, as one transaction.
fn nft(script: &str) -> Result<(), String> {
    let mut c = Command::new(tool("nft")?)
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("nft: {e}"))?;
    let _ = c.stdin.take().unwrap().write_all(script.as_bytes());
    let o = c.wait_with_output().map_err(|e| format!("nft: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "nft: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(())
}

fn table_exists() -> Result<bool, String> {
    Ok(Command::new(tool("nft")?)
        .args(["list", "table", "inet", "ohmlatl"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("nft: {e}"))?
        .success())
}

/// The chains and map elements of VM `n`. `dns` is the (udp, tcp) port of the relay, with
/// `internet: true`.
pub fn vm_rules(
    n: u32,
    uid: u32,
    gw: Ipv4Addr,
    guest: Ipv4Addr,
    dns: Option<(u16, u16)>,
) -> String {
    let t = "inet ohmlatl";
    let mut s = String::new();
    for (c, _) in CHAINS {
        s += &format!("add chain {t} vm{n}_{c}\n");
    }
    s += &format!("add rule {t} vm{n}_raw ip saddr != {guest} drop\n");
    if let Some((udp, tcp)) = dns {
        s += &format!("add rule {t} vm{n}_nat ip daddr {gw} udp dport 53 redirect to :{udp}\n");
        s += &format!("add rule {t} vm{n}_nat ip daddr {gw} tcp dport 53 redirect to :{tcp}\n");
    }
    s += &format!("add rule {t} vm{n}_in ct state established,related accept\n");
    if dns.is_some() {
        s += &format!(
            "add rule {t} vm{n}_in ip daddr {gw} ct status dnat ct original proto-dst {{ 53 }} accept\n"
        );
    }
    let fwd = if dns.is_some() { "accept" } else { "reject" };
    s += &format!("add rule {t} vm{n}_fwd {fwd}\n");
    s += &format!("add rule {t} vm{n}_out meta skuid != {{ 0, {uid} }} reject\n");
    for (c, map) in CHAINS {
        s += &format!("add element {t} {map} {{ \"ohm{n}\" : jump vm{n}_{c} }}\n");
    }
    s += &format!("add element {t} gws {{ {gw} }}\n");
    s
}

/// The `blocked` set: the fixed ranges plus the networks the host is connected to now.
pub fn blocked_script(host: &Host) -> String {
    let mut elems: Vec<String> = FIXED_BLOCKED.iter().map(|s| s.to_string()).collect();
    for (dev, a) in &host.addrs {
        let net = show(*a);
        if !dev.starts_with("ohm") && !elems.contains(&net) {
            elems.push(net);
        }
    }
    format!(
        "flush set inet ohmlatl blocked\nadd element inet ohmlatl blocked {{ {} }}\n",
        elems.join(", ")
    )
}

fn refresh_blocked() -> Result<(), String> {
    nft(&blocked_script(&Host::read()?))
}

/// `ip_forward=1`; if it was off, the `inet ohmlatl-fwguard` lock goes in first, once per boot.
fn forward_on() -> Result<(), String> {
    const F: &str = "/proc/sys/net/ipv4/ip_forward";
    if fs::read_to_string(F)
        .map_err(|e| format!("{F}: {e}"))?
        .trim()
        == "1"
    {
        return Ok(());
    }
    let was_off = Path::new(NET).join("forward-was-off");
    if !was_off.exists() {
        nft(FWGUARD)?;
        fs::write(&was_off, "").map_err(|e| format!("{}: {e}", was_off.display()))?;
    }
    fs::write(F, "1").map_err(|e| format!("{F}: {e}"))
}

pub struct Net {
    pub tap: String,
    pub gw: Ipv4Addr,
    pub guest: Ipv4Addr,
    /// The DNS relay sockets, with `internet: true`.
    pub dns: Option<(UdpSocket, TcpListener)>,
}

/// Supervisor step 2, under the network lock: pool check, `/30` and TAP, recorded before
/// anything is created; TAP; `ip_forward`; relay listeners; nft. A failure at any point leaves
/// the record the cleanup needs.
pub fn setup(id: &str, uid: u32, pool: &str, internet: bool) -> Result<Net, String> {
    let pool = parse_cidr(pool).filter(|c| c.1 <= 30).ok_or(format!(
        "invalid pool {pool} (want a CIDR of /30 or larger)"
    ))?;
    let _lock = lock()?;
    let host = Host::read()?;
    if let Some(c) = conflict(pool, &host) {
        return Err(format!(
            "the host uses the VM pool {}: {c}; set another one, e.g. OHMLATL_POOL=10.213.0.0/16",
            show(pool)
        ));
    }
    let (n, subnet) = allocate(pool, &host, &registries())?;
    let tap = format!("ohm{n}");
    let (gw, guest) = (Ipv4Addr::from(subnet.0 + 1), Ipv4Addr::from(subnet.0 + 2));
    let reg = Path::new(NET).join(id);
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&reg)
        .and_then(|mut f| writeln!(f, "{tap} {}", show(subnet)))
        .map_err(|e| format!("{}: {e}", reg.display()))?;

    ip(&[
        "tuntap",
        "add",
        "dev",
        &tap,
        "mode",
        "tap",
        "user",
        &uid.to_string(),
    ])?;
    ip(&["addr", "add", &format!("{gw}/30"), "dev", &tap])?;
    if Path::new("/proc/sys/net/ipv6").exists() {
        let p = format!("/proc/sys/net/ipv6/conf/{tap}/disable_ipv6");
        fs::write(&p, "1").map_err(|e| format!("{p}: {e}"))?;
    }
    ip(&["link", "set", &tap, "up"])?;
    forward_on()?;

    let dns = if internet {
        let udp = UdpSocket::from(listener(gw, &tap, libc::SOCK_DGRAM)?);
        let tcp = TcpListener::from(listener(gw, &tap, libc::SOCK_STREAM)?);
        Some((udp, tcp))
    } else {
        None
    };
    let ports = match &dns {
        Some((u, t)) => Some((
            u.local_addr().map_err(|e| e.to_string())?.port(),
            t.local_addr().map_err(|e| e.to_string())?.port(),
        )),
        None => None,
    };
    if !table_exists()? {
        nft(TABLE)?;
    }
    refresh_blocked()?;
    nft(&vm_rules(n, uid, gw, guest, ports))?;
    Ok(Net {
        tap,
        gw,
        guest,
        dns,
    })
}

/// Cleanup step 3, under the network lock. Everything that is already gone is skipped.
pub fn teardown(id: &str) {
    let lock = match lock() {
        Ok(l) => l,
        Err(e) => return log(format!("cleanup: {e}")),
    };
    let reg = Path::new(NET).join(id);
    if let Some((tap, subnet)) = fs::read_to_string(&reg).ok().and_then(|t| parse_reg(&t)) {
        if Path::new("/sys/class/net").join(&tap).exists()
            && let Err(e) = ip(&["link", "del", &tap])
        {
            log(format!("cleanup: {e}"));
        }
        let n = &tap[3..];
        let gw = Ipv4Addr::from(subnet.0 + 1);
        // One command each: a transaction would fail whole on the first missing element.
        let mut cmds: Vec<String> = CHAINS
            .iter()
            .map(|(_, map)| format!("delete element inet ohmlatl {map} {{ \"{tap}\" }}"))
            .collect();
        cmds.push(format!("delete element inet ohmlatl gws {{ {gw} }}"));
        cmds.extend(
            CHAINS
                .iter()
                .map(|(c, _)| format!("delete chain inet ohmlatl vm{n}_{c}")),
        );
        if table_exists().unwrap_or(false) {
            for c in cmds {
                let _ = nft(&c);
            }
        }
    }
    let any_tap = fs::read_dir("/sys/class/net")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("ohm"));
    if !any_tap
        && table_exists().unwrap_or(false)
        && let Err(e) = nft("delete table inet ohmlatl")
    {
        log(format!("cleanup: {e}"));
    }
    let _ = fs::remove_file(&reg);
    drop(lock);
}

/// A socket on `<gw>:0` bound to the TAP (`SO_BINDTODEVICE`).
fn listener(gw: Ipv4Addr, tap: &str, kind: i32) -> Result<OwnedFd, String> {
    let fd = unsafe { libc::socket(libc::AF_INET, kind | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return last_err("socket");
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let r = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            tap.as_ptr().cast(),
            tap.len() as libc::socklen_t,
        )
    };
    if r != 0 {
        return last_err(format!("SO_BINDTODEVICE {tap}"));
    }
    let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    sa.sin_family = libc::AF_INET as libc::sa_family_t;
    sa.sin_addr.s_addr = u32::from(gw).to_be();
    let r = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&sa as *const libc::sockaddr_in).cast(),
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if r != 0 {
        return last_err(format!("bind {gw}:0"));
    }
    if kind == libc::SOCK_STREAM && unsafe { libc::listen(fd.as_raw_fd(), 64) } != 0 {
        return last_err("listen");
    }
    Ok(fd)
}

/// The host's nameservers, read again when /etc/resolv.conf changes.
fn nameservers() -> Vec<SocketAddr> {
    static CACHE: Mutex<(Option<SystemTime>, Vec<SocketAddr>)> = Mutex::new((None, vec![]));
    let mtime = fs::metadata("/etc/resolv.conf")
        .and_then(|m| m.modified())
        .ok();
    let mut c = CACHE.lock().unwrap();
    if c.0 != mtime || c.1.is_empty() {
        let text = fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        c.1 = text
            .lines()
            .filter_map(|l| l.strip_prefix("nameserver"))
            .filter_map(|v| v.trim().parse::<IpAddr>().ok())
            .map(|ip| SocketAddr::new(ip, 53))
            .collect();
        c.0 = mtime;
    }
    c.1.clone()
}

fn query_udp(q: &[u8]) -> Option<Vec<u8>> {
    for ns in nameservers() {
        let local = if ns.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let Ok(s) = UdpSocket::bind(local) else {
            continue;
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
        let mut buf = vec![0u8; 65535];
        if s.connect(ns).is_ok()
            && s.send(q).is_ok()
            && let Ok(n) = s.recv(&mut buf)
        {
            buf.truncate(n);
            return Some(buf);
        }
    }
    None
}

/// Counts a running query; `None` past the limit.
fn slot(inflight: &Arc<AtomicUsize>) -> Option<Slot> {
    if inflight.fetch_add(1, Ordering::SeqCst) >= DNS_INFLIGHT {
        inflight.fetch_sub(1, Ordering::SeqCst);
        return None;
    }
    Some(Slot(inflight.clone()))
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The DNS relay: 53/udp and 53/tcp of the gateway (redirected to these sockets) to the
/// host's resolvers. Only the guest's address is served.
pub fn serve_dns(udp: UdpSocket, tcp: TcpListener, guest: Ipv4Addr) {
    let inflight = Arc::new(AtomicUsize::new(0));
    let udp = Arc::new(udp);
    let inf = inflight.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65535];
        while let Ok((n, src)) = udp.recv_from(&mut buf) {
            if src.ip() != IpAddr::V4(guest) {
                continue;
            }
            let Some(slot) = slot(&inf) else { continue };
            let (q, udp) = (buf[..n].to_vec(), udp.clone());
            std::thread::spawn(move || {
                if let Some(r) = query_udp(&q) {
                    let _ = udp.send_to(&r, src);
                }
                drop(slot);
            });
        }
    });
    std::thread::spawn(move || {
        for c in tcp.incoming().flatten() {
            if !c.peer_addr().is_ok_and(|a| a.ip() == IpAddr::V4(guest)) {
                continue;
            }
            let Some(slot) = slot(&inflight) else {
                continue;
            };
            std::thread::spawn(move || {
                for ns in nameservers() {
                    if let Ok(up) = TcpStream::connect_timeout(&ns, Duration::from_secs(3)) {
                        pipe(c, up);
                        break;
                    }
                }
                drop(slot);
            });
        }
    });
}

/// Copies both ways until either side closes or stays idle for 30 s.
fn pipe(a: TcpStream, b: TcpStream) {
    let t = Some(Duration::from_secs(30));
    for s in [&a, &b] {
        let _ = s.set_read_timeout(t);
        let _ = s.set_write_timeout(t);
    }
    let (Ok(mut a2), Ok(mut b2)) = (a.try_clone(), b.try_clone()) else {
        return;
    };
    let h = std::thread::spawn(move || {
        let _ = io::copy(&mut a2, &mut b2);
        let _ = b2.shutdown(Shutdown::Both);
    });
    let (mut a, mut b) = (a, b);
    let _ = io::copy(&mut b, &mut a);
    let _ = a.shutdown(Shutdown::Both);
    let _ = h.join();
}

/// Recomputes `blocked` on every route or address change of the host (netlink), under the
/// network lock, for as long as the supervisor runs.
pub fn watch_routes() {
    std::thread::spawn(|| {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return log(format!("netlink: {}", io::Error::last_os_error()));
        }
        let mut f = unsafe { File::from_raw_fd(fd) };
        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        sa.nl_groups = (libc::RTMGRP_IPV4_IFADDR | libc::RTMGRP_IPV4_ROUTE) as u32;
        let r = unsafe {
            libc::bind(
                fd,
                (&sa as *const libc::sockaddr_nl).cast(),
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if r != 0 {
            return log(format!("netlink bind: {}", io::Error::last_os_error()));
        }
        let mut buf = vec![0u8; 1 << 16];
        loop {
            match f.read(&mut buf) {
                Ok(_) => {}
                // A burst overflowed the socket: something changed all the same.
                Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => {}
                Err(e) => return log(format!("netlink: {e}")),
            }
            // A network change comes as a burst of messages: let it settle, then drain it.
            std::thread::sleep(Duration::from_millis(500));
            while unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT) }
                > 0
            {}
            if let Err(e) = lock().and_then(|_l| refresh_blocked()) {
                log(format!("blocked: {e}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        parse_cidr(s).unwrap()
    }

    const ADDRS: &str = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever
3: wlan0    inet 192.168.0.105/24 brd 192.168.0.255 scope global dynamic noprefixroute wlan0\\       valid_lft 6000sec
7: CloudflareWARP    inet 172.16.0.2/32 scope global CloudflareWARP\\       valid_lft forever
9: ohm0    inet 172.31.0.1/30 scope global ohm0\\       valid_lft forever
";

    const ROUTES: &str = "default via 192.168.0.1 dev wlan0 proto dhcp src 192.168.0.105 metric 600
172.17.0.0/16 dev docker0 proto kernel scope link src 172.17.0.1 linkdown
172.31.0.0/30 dev ohm0 proto kernel scope link src 172.31.0.1
172.0.0.0/12 dev CloudflareWARP table 65743 proto static scope link
local 172.31.0.1 dev ohm0 table local proto kernel scope host src 172.31.0.1
broadcast 192.168.0.255 dev wlan0 table local proto kernel scope link src 192.168.0.105
";

    fn host() -> Host {
        Host {
            links: ["lo", "wlan0", "CloudflareWARP", "ohm0"]
                .map(String::from)
                .to_vec(),
            addrs: parse_addrs(ADDRS),
            routes: parse_routes(ROUTES),
        }
    }

    #[test]
    fn parses_ip_output() {
        let h = host();
        assert_eq!(h.addrs.len(), 4);
        assert_eq!(h.addrs[1], ("wlan0".into(), c("192.168.0.105/24")));
        assert!(
            h.routes.iter().all(|(_, r)| r.1 != 0),
            "default route left out"
        );
        assert!(h.routes.contains(&("ohm0".into(), c("172.31.0.1/32"))));
        assert!(
            h.routes
                .contains(&("CloudflareWARP".into(), c("172.0.0.0/12")))
        );
    }

    #[test]
    fn pool_conflicts() {
        let pool = c("172.31.0.0/16");
        // ohm* routes and addresses, the default route and a route around the pool are fine.
        assert_eq!(conflict(pool, &host()), None);
        let mut h = host();
        h.routes.push(("tun0".into(), c("172.31.8.0/24")));
        assert_eq!(conflict(pool, &h).unwrap(), "route 172.31.8.0/24 via tun0");
        let mut h = host();
        h.addrs.push(("eth1".into(), c("172.31.200.7/8")));
        assert_eq!(conflict(pool, &h).unwrap(), "address 172.31.200.7 on eth1");
        let mut h = host();
        h.routes.push(("".into(), c("172.31.0.0/16")));
        assert!(conflict(pool, &h).is_some(), "a blackhole route counts too");
        assert_eq!(conflict(c("10.213.0.0/16"), &host()), None);
    }

    #[test]
    fn allocation() {
        let pool = c("172.31.0.0/16");
        let mut h = host();
        // ohm0 exists with 172.31.0.0/30 (a leftover): next TAP and next /30.
        assert_eq!(allocate(pool, &h, &[]).unwrap(), (1, c("172.31.0.4/30")));
        // Registries hold their TAP and /30 even when the TAP is gone.
        let regs = [("ohm1".to_string(), c("172.31.0.4/30"))];
        assert_eq!(allocate(pool, &h, &regs).unwrap(), (2, c("172.31.0.8/30")));
        // A hole left by a stopped VM is reused.
        h.links.retain(|l| l != "ohm0");
        h.addrs.retain(|(d, _)| d != "ohm0");
        assert_eq!(allocate(pool, &h, &regs).unwrap(), (0, c("172.31.0.0/30")));
        // A pool of a single /30, used up.
        let tiny = c("10.9.0.0/30");
        assert_eq!(allocate(tiny, &h, &[]).unwrap(), (0, c("10.9.0.0/30")));
        let regs = [("ohm0".to_string(), c("10.9.0.0/30"))];
        assert!(
            allocate(tiny, &h, &regs)
                .unwrap_err()
                .contains("no free /30")
        );
    }

    #[test]
    fn registry_format() {
        assert_eq!(
            parse_reg("ohm3 172.31.0.12/30\n"),
            Some(("ohm3".into(), c("172.31.0.12/30")))
        );
        assert_eq!(parse_reg(""), None);
        assert_eq!(parse_reg("eth0 172.31.0.12/30"), None);
        assert_eq!(parse_reg("ohm 172.31.0.12/30"), None);
        assert_eq!(parse_reg("ohm1 172.31.0.12/24"), None);
    }

    #[test]
    fn rules_per_vm() {
        let gw = Ipv4Addr::new(172, 31, 0, 1);
        let guest = Ipv4Addr::new(172, 31, 0, 2);
        assert_eq!(
            vm_rules(0, 1000, gw, guest, Some((40001, 40002))),
            "add chain inet ohmlatl vm0_raw
add chain inet ohmlatl vm0_nat
add chain inet ohmlatl vm0_in
add chain inet ohmlatl vm0_fwd
add chain inet ohmlatl vm0_out
add rule inet ohmlatl vm0_raw ip saddr != 172.31.0.2 drop
add rule inet ohmlatl vm0_nat ip daddr 172.31.0.1 udp dport 53 redirect to :40001
add rule inet ohmlatl vm0_nat ip daddr 172.31.0.1 tcp dport 53 redirect to :40002
add rule inet ohmlatl vm0_in ct state established,related accept
add rule inet ohmlatl vm0_in ip daddr 172.31.0.1 ct status dnat ct original proto-dst { 53 } accept
add rule inet ohmlatl vm0_fwd accept
add rule inet ohmlatl vm0_out meta skuid != { 0, 1000 } reject
add element inet ohmlatl vm_iif_raw { \"ohm0\" : jump vm0_raw }
add element inet ohmlatl vm_iif_nat { \"ohm0\" : jump vm0_nat }
add element inet ohmlatl vm_iif_in { \"ohm0\" : jump vm0_in }
add element inet ohmlatl vm_iif_fwd { \"ohm0\" : jump vm0_fwd }
add element inet ohmlatl vm_oif_out { \"ohm0\" : jump vm0_out }
add element inet ohmlatl gws { 172.31.0.1 }
"
        );
        // internet: false: no DNS, forward rejected.
        let off = vm_rules(3, 1001, gw, guest, None);
        assert!(!off.contains("redirect") && !off.contains("proto-dst"));
        assert!(off.contains("add rule inet ohmlatl vm3_fwd reject\n"));
        assert!(off.contains("meta skuid != { 0, 1001 } reject"));
    }

    #[test]
    fn blocked_set() {
        assert_eq!(
            blocked_script(&host()),
            "flush set inet ohmlatl blocked
add element inet ohmlatl blocked { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, 100.64.0.0/10, 127.0.0.0/8, 192.168.0.0/24, 172.16.0.2/32 }
"
        );
    }
}
