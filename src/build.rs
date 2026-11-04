//! `ohmlatl build` (docs/TECHNICAL.md, "Build").

use crate::assets;
use crate::doctor;
use crate::home;
use crate::manifest;
use crate::supervise::{valid_name, valid_tag};
use crate::util::{self, cache_home, check_interrupted, gib, sudo, tool, try_lock};
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
/// LUKS2 data offset, fixed since the build: the file is the disk size plus this.
pub const LUKS_OFFSET: u64 = 16 * MIB;
/// Fixed PBKDF (no benchmark): unlocking in the guest does not depend on the build machine.
pub const PBKDF: [&str; 8] = [
    "--pbkdf",
    "argon2id",
    "--pbkdf-memory",
    "262144",
    "--pbkdf-parallel",
    "1",
    "--pbkdf-force-iterations",
    "4",
];
/// base_features + ext4 features of assets/mke2fs.conf.
const FEATURES: &str = "sparse_super,large_file,filetype,resize_inode,dir_index,ext_attr,has_journal,extent,huge_file,flex_bg,metadata_csum,64bit,dir_nlink,extra_isize";

/// Export, validation and adjustments, inside `podman unshare` (docs/TECHNICAL.md, step 2).
const EXPORT: &str = r#"
umask 022
fail() { echo "[build] $*" >&2; exit 1; }
warn() { echo "[build] warning: $*" >&2; }
# A path of the image with every symlink resolved inside it.
inside() { chroot "$root" readlink -e "$1" 2>/dev/null; }
root="$BUILD/root"; mkdir -m 0755 "$root"
podman export "$CID" | tar -x --same-owner --numeric-owner \
  --xattrs --xattrs-include="*" --xattrs-exclude="security.selinux" -C "$root"
# Before reading anything in /etc: a symlink there would point at the host.
for p in etc etc/ssh etc/systemd etc/systemd/system etc/NetworkManager etc/NetworkManager/conf.d; do
  [ -L "$root/$p" ] && fail "symlink at /$p in the image"
done
init=$(inside /sbin/init) && [ "${init##*/}" = systemd ] && [ -x "$root$init" ] \
  || fail "/sbin/init in the image is not systemd; see examples/kali"
sshd=$(inside /usr/sbin/sshd) && [ -x "$root$sshd" ] \
  || fail "the image has no /usr/sbin/sshd; see examples/kali"
ent=$(awk -F: -v u="$USER_" '$1 == u { print $3 ":" $4 ":" $6; exit }' "$root/etc/passwd")
[ -n "$ent" ] || fail "user $USER_ is not in /etc/passwd of the image"
[ "${ent%:*}" = "$UID_:$GID_" ] \
  || fail "user $USER_ has uid:gid ${ent%:*} in the image, the manifest says $UID_:$GID_"
h=$(inside "${ent##*:}") && [ -d "$root$h" ] || fail "home ${ent##*:} of $USER_ does not exist in the image"
[ "$(stat -c %u "$root$h")" = "$UID_" ] || fail "home ${ent##*:} of $USER_ is not owned by uid $UID_"
sftp=$(cat "$root"/etc/ssh/sshd_config.d/*.conf "$root/etc/ssh/sshd_config" 2>/dev/null \
  | awk 'tolower($1) == "subsystem" && $2 == "sftp" { print $3; exit }')
case $sftp in
  internal-sftp) ;;
  "") warn "sshd_config has no sftp subsystem: ohmlatl cp will not work" ;;
  *) p=$(inside "$sftp") && [ -x "$root$p" ] \
       || warn "sftp-server $sftp is missing in the image: ohmlatl cp will not work" ;;
esac
pw=$(awk -F: -v u="$USER_" '$1 == u { print $2; exit }' "$root/etc/shadow" 2>/dev/null || true)
case $pw in
  ""|"!"*|"*"*) [ -s "$root$h/.ssh/authorized_keys" ] \
    || warn "user $USER_ has no password and no ~/.ssh/authorized_keys: ohmlatl shell will not log in (the console still works)" ;;
esac
rm -f "$root/etc/machine-id" "$root/etc/hostname" "$root/etc/hosts" \
      "$root/etc/NetworkManager/conf.d/ohmlatl.conf" \
      "$root/etc/systemd/system/serial-getty@ttyS0.service.d/autologin.conf"
: > "$root/etc/machine-id"
printf "%s\n" "$NAME" > "$root/etc/hostname"
printf "127.0.0.1 localhost %s\n" "$NAME" > "$root/etc/hosts"
rm -f "$root/etc/resolv.conf"; : > "$root/etc/resolv.conf"
ls "$root"/etc/ssh/ssh_host_*_key >/dev/null 2>&1 || ssh-keygen -A -f "$root" >&2
install -d -m 0755 -o 0 -g 0 "$root/etc/NetworkManager/conf.d"
printf "[main]\ndns=none\n\n[keyfile]\nunmanaged-devices=interface-name:eth0\n" \
  > "$root/etc/NetworkManager/conf.d/ohmlatl.conf"
for u in systemd-networkd.service systemd-networkd-wait-online.service \
         systemd-resolved.service; do
  ln -sf /dev/null "$root/etc/systemd/system/$u"
done
ln -sf /lib/systemd/system/reboot.target "$root/etc/systemd/system/poweroff.target"
ln -sf /lib/systemd/system/reboot.target "$root/etc/systemd/system/halt.target"
d="$root/etc/systemd/system/serial-getty@ttyS0.service.d"
install -d -m 0755 -o 0 -g 0 "$d"
printf "[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin %s --keep-baud 115200,57600,38400,9600 %%I \$TERM\n" \
  "$USER_" > "$d/autologin.conf"
chmod 0755 "$root"
chmod 0444 "$root/etc/machine-id"
chmod 0644 "$root/etc/hostname" "$root/etc/hosts" "$root/etc/resolv.conf" \
  "$root/etc/NetworkManager/conf.d/ohmlatl.conf" "$d/autologin.conf"
du -sB1 "$root"; find "$root" -xdev | wc -l
"#;

pub struct Opts {
    pub file: Option<String>,
    pub from: Option<String>,
    pub tag: Option<String>,
    pub size: Option<u64>,
}

fn say(msg: impl std::fmt::Display) {
    println!("[build] {msg}");
}

/// The disk the VM sees: `--size`, or max(32 GiB, 2 x content + 1 GiB), rounded up to 1 MiB.
pub fn disk_size(requested: Option<u64>, content: u64) -> u64 {
    requested
        .unwrap_or((32 * GIB).max(2 * content + GIB))
        .div_ceil(MIB)
        * MIB
}

/// `-N` for mkfs when the default (one inode per 16 KiB) is less than twice the files.
fn inodes(size: u64, files: u64) -> Option<u64> {
    (size / 16384 < 2 * files).then_some(2 * files)
}

/// What the build writes to the drive: content, inode tables (256 bytes each), the journal
/// (mke2fs picks about size/128, at most 1 GiB) and the LUKS header.
/// shortcut: an estimate without ext4 bitmaps and directories; enough for a free-space check.
fn written(content: u64, size: u64, inodes: u64) -> u64 {
    content + inodes * 256 + (size / 128).min(GIB) + LUKS_OFFSET
}

/// What the trap of the build undoes (docs/TECHNICAL.md, step 8).
struct Cleanup {
    build: PathBuf,
    cid: Option<String>,
    /// The image `podman build` made (with -f): removed if nothing else tagged it.
    built: Option<String>,
    mapper: Option<String>,
    part: Option<PathBuf>,
    sudo: Option<util::Sudo>,
}

impl Cleanup {
    fn run(mut self) {
        if self.build.exists()
            && let Ok(mut c) = assets::podman()
        {
            let _ = c.args(["unshare", "rm", "-rf"]).arg(&self.build).status();
        }
        if let Some(m) = self.mapper.take() {
            let _ = sudo(
                &[
                    OsStr::new("cryptsetup"),
                    OsStr::new("close"),
                    OsStr::new(&m),
                ],
                None,
            );
        }
        if let Some(p) = self.part.take() {
            let _ = fs::remove_dir_all(p);
        }
        if let Some(cid) = self.cid.take()
            && let Ok(mut c) = assets::podman()
        {
            let _ = c.args(["rm", "-f", &cid]).stdout(Stdio::null()).status();
        }
        if let Some(id) = self.built.take()
            && let Ok(mut c) = assets::podman()
            && c.args(["image", "inspect", "--format", "{{len .RepoTags}}", &id])
                .output()
                .is_ok_and(|o| o.status.success() && o.stdout.trim_ascii() == b"0")
            && let Ok(mut c) = assets::podman()
        {
            let _ = c.args(["rmi", &id]).stdout(Stdio::null()).status();
        }
        self.sudo.take();
    }
}

pub fn build(home_flag: Option<&Path>, o: Opts) -> Result<(), String> {
    let text = fs::read_to_string("ohmlatl.yaml")
        .map_err(|e| format!("ohmlatl.yaml in the current directory: {e}"))?;
    let mut m = manifest::parse(&text).map_err(|e| format!("ohmlatl.yaml: {e}"))?;
    if let Some(t) = &o.tag {
        // -t wins over name and build.tag; without a tag, latest.
        let (n, t) = t.split_once(':').unwrap_or((t, "latest"));
        (m.name, m.tag) = (n.to_string(), Some(t.to_string()));
    }
    let (name, tag) = (m.name.clone(), m.tag.get_or_insert("latest".into()).clone());
    if !valid_name(&name) || !valid_tag(&tag) {
        return Err(format!(
            "invalid image {name}:{tag} (name ^[a-z][a-z0-9-]{{0,30}}$, tag ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$)"
        ));
    }
    let containerfile = match (&o.file, &o.from) {
        (Some(_), Some(_)) => return Err("-f and --from exclude each other".into()),
        (_, Some(_)) => None,
        (f, None) => Some(f.clone().or(m.containerfile.clone()).ok_or(
            "no Containerfile: pass -f, set build.containerfile in ohmlatl.yaml, or use --from",
        )?),
    };
    let home = home::discover(home_flag)?;
    home::check_owner(&home)?;
    let images = home.join("images");
    let dest = images.join(&name).join(&tag);
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(format!(
            "{name}:{tag} already exists ({}); a build never overwrites a computer, use another tag",
            dest.display()
        ));
    }
    for t in [
        "mkfs.ext4",
        "e2fsck",
        "cryptsetup",
        "udevadm",
        "ssh-keygen",
        "sudo",
    ] {
        tool(t)?;
    }
    // Before any sudo: nobody else may swap what root is about to touch.
    let exe = fs::read_link("/proc/self/exe").map_err(|e| format!("/proc/self/exe: {e}"))?;
    home::check_not_foreign_writable(&exe)?;
    home::check_not_foreign_writable(&home)?;

    util::catch_signals();
    let mut c = Cleanup {
        build: cache_home()
            .map_err(|e| e.to_string())?
            .join("ohmlatl/build")
            .join(std::process::id().to_string()),
        cid: None,
        built: None,
        mapper: None,
        part: None,
        sudo: None,
    };
    let r = steps(&mut c, &m, &o, containerfile.as_deref(), &images, &dest);
    c.run();
    r
}

fn steps(
    c: &mut Cleanup,
    m: &manifest::Manifest,
    o: &Opts,
    containerfile: Option<&str>,
    images: &Path,
    dest: &Path,
) -> Result<(), String> {
    let (name, tag) = (&m.name, m.tag.as_deref().unwrap());
    // 1. podman build (with -f), then always podman create.
    if c.build.exists() {
        // Left by a dead build that had our pid.
        util::run(
            assets::podman()
                .map_err(|e| e.to_string())?
                .args(["unshare", "rm", "-rf"])
                .arg(&c.build),
            None,
        )?;
    }
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&c.build)
        .map_err(|e| format!("{}: {e}", c.build.display()))?;
    let image = match containerfile {
        Some(f) => {
            let iid = c.build.join("iid");
            util::run(
                assets::podman()
                    .map_err(|e| e.to_string())?
                    .args(["build", "-f", f, "--iidfile"])
                    .arg(&iid)
                    .arg("."),
                None,
            )?;
            let id = fs::read_to_string(&iid).map_err(|e| format!("{}: {e}", iid.display()))?;
            let id = id.trim().trim_start_matches("sha256:").to_string();
            c.built = Some(id.clone());
            id
        }
        None => o.from.clone().unwrap(),
    };
    check_interrupted()?;
    let out = assets::podman()
        .map_err(|e| e.to_string())?
        .args(["create", "--pull=never", &image, "/bin/true"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("podman create: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "podman create {image} failed; --from uses local images only: pull remote ones first with `ohmlatl podman pull docker.io/...`"
        ));
    }
    c.cid = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());

    // 2. Export, validation and adjustments.
    say("exporting rootfs (podman unshare)");
    let out = assets::podman()
        .map_err(|e| e.to_string())?
        .args(["unshare", "sh", "-euc", EXPORT])
        .env("BUILD", &c.build)
        .env("CID", c.cid.as_ref().unwrap())
        .env("NAME", name)
        .env("USER_", &m.user.name)
        .env("UID_", m.user.uid.to_string())
        .env("GID_", m.user.gid.to_string())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("podman unshare: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if !out.status.success() || lines.len() < 2 {
        return Err("exporting the rootfs failed".into());
    }
    let content: u64 = lines[lines.len() - 2]
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or("du: unexpected output")?;
    let files: u64 = lines[lines.len() - 1]
        .trim()
        .parse()
        .map_err(|_| "find: unexpected output")?;
    check_interrupted()?;

    // 3. Size, inodes and free space.
    say(format!("rootfs content: {} ({files} files)", gib(content)));
    let size = disk_size(o.size, content);
    let n = inodes(size, files);
    say(format!(
        "size: {}, sparse ({})",
        gib(size),
        if o.size.is_some() {
            "--size"
        } else {
            "default; --size to change"
        }
    ));
    let need = written(content, size, n.unwrap_or(size / 16384));
    if size < need - LUKS_OFFSET {
        return Err(format!(
            "--size {} is too small: {} of content plus ext4 metadata need about {}",
            gib(size),
            gib(content),
            gib(need - LUKS_OFFSET)
        ));
    }
    let free = util::free_bytes(images).map_err(|e| format!("{}: {e}", images.display()))?;
    if need > free {
        return Err(format!(
            "the build writes about {} to the drive, {} free",
            gib(need),
            gib(free)
        ));
    }

    // 4. Passphrase, once.
    let pass = util::passphrase("Rootfs passphrase", true)?;

    // 5. LUKS, under the build lock.
    c.sudo = Some(util::Sudo::start()?);
    let dir = images.join(name);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let lock_path = dir.join(format!(".{tag}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|e| format!("{}: {e}", lock_path.display()))?;
    if !try_lock(&lock) {
        return Err(format!("another build of {name}:{tag} is running"));
    }
    if fs::symlink_metadata(dest).is_ok() {
        return Err(format!("{name}:{tag} already exists ({})", dest.display()));
    }
    let part = dir.join(format!(".{tag}.partial"));
    if fs::symlink_metadata(&part).is_ok() {
        say(format!("removing leftover {}", part.display()));
        doctor::clean_partial(&part, &doctor::mappers())
            .map_err(|e| format!("{}: {e}", part.display()))?;
    }
    DirBuilder::new()
        .mode(0o700)
        .create(&part)
        .map_err(|e| format!("{}: {e}", part.display()))?;
    c.part = Some(part.clone());
    let rootfs = part.join("rootfs.luks");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&rootfs)
        .and_then(|f| f.set_len(size + LUKS_OFFSET))
        .map_err(|e| format!("{}: {e}", rootfs.display()))?;
    say("luks2 (aes-xts-plain64)");
    let cryptsetup = tool("cryptsetup")?;
    let mut args: Vec<&OsStr> = [
        cryptsetup.as_os_str(),
        "luksFormat".as_ref(),
        "--batch-mode".as_ref(),
        "--type".as_ref(),
        "luks2".as_ref(),
        "--cipher".as_ref(),
        "aes-xts-plain64".as_ref(),
        "--key-size".as_ref(),
        "512".as_ref(),
        "--sector-size".as_ref(),
        "4096".as_ref(),
        "--offset".as_ref(),
        "32768".as_ref(),
    ]
    .to_vec();
    args.extend(PBKDF.iter().map(OsStr::new));
    args.extend(["--key-file=-".as_ref(), rootfs.as_os_str()]);
    sudo(&args, Some(&pass))?;
    let mapper = format!("ohmlatl-build-{}", std::process::id());
    sudo(
        &[
            cryptsetup.as_os_str(),
            "open".as_ref(),
            "--key-file=-".as_ref(),
            rootfs.as_os_str(),
            mapper.as_ref(),
        ],
        Some(&pass),
    )?;
    c.mapper = Some(mapper.clone());
    check_interrupted()?;

    // 6. ext4 straight on the mapper, inside podman unshare.
    say("mkfs.ext4 + e2fsck");
    let dev = PathBuf::from("/dev/mapper").join(&mapper);
    // Only our node: the whole queue can stay busy for long (nfsrahead on every FUSE mount).
    let exists = format!("--exit-if-exists={}", dev.display());
    sudo(
        &[
            tool("udevadm")?.as_os_str(),
            "settle".as_ref(),
            exists.as_ref(),
        ],
        None,
    )?;
    let node = fs::canonicalize(&dev).map_err(|e| format!("{}: {e}", dev.display()))?;
    sudo(
        &[
            "chown".as_ref(),
            util::uid().to_string().as_ref(),
            node.as_os_str(),
        ],
        None,
    )?;
    let mke2fs_conf = assets::extract_build()
        .map_err(|e| e.to_string())?
        .join("mke2fs.conf");
    let mut mkfs = assets::podman().map_err(|e| e.to_string())?;
    mkfs.args(["unshare", "env"])
        .arg(format!("MKE2FS_CONFIG={}", mke2fs_conf.display()))
        .arg(tool("mkfs.ext4")?)
        .args([
            "-q", "-F", "-t", "ext4", "-O", FEATURES, "-L", "rootfs", "-m", "0",
        ])
        .args(["-E", "lazy_itable_init=0,lazy_journal_init=0,nodiscard"]);
    if let Some(n) = n {
        mkfs.args(["-N", &n.to_string()]);
    }
    util::run(mkfs.arg("-d").arg(c.build.join("root")).arg(&dev), None)
        .map_err(|_| "mkfs.ext4 failed (see its message above)")?;
    sudo(
        &[tool("e2fsck")?.as_os_str(), "-fn".as_ref(), dev.as_os_str()],
        None,
    )?;
    sudo(
        &[cryptsetup.as_os_str(), "close".as_ref(), mapper.as_ref()],
        None,
    )?;
    c.mapper = None;
    check_interrupted()?;

    // 7. manifest.yaml with the final name and tag, then rename.
    let mf = part.join("manifest.yaml");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&mf)
        .and_then(|mut f| {
            f.write_all(m.to_yaml().as_bytes())?;
            f.sync_all()
        })
        .map_err(|e| format!("{}: {e}", mf.display()))?;
    fs::File::open(&rootfs)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("{}: {e}", rootfs.display()))?;
    fs::rename(&part, dest).map_err(|e| format!("rename to {}: {e}", dest.display()))?;
    c.part = None;
    let _ = fs::File::open(&dir).and_then(|d| d.sync_all());
    drop(lock);

    let st = fs::metadata(dest.join("rootfs.luks")).map_err(|e| e.to_string())?;
    say(format!("disk image {name}:{tag}"));
    println!("  rootfs: {}", dest.join("rootfs.luks").display());
    println!(
        "  size:   {} ({} on disk)",
        gib(size),
        gib(st.blocks() * 512)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_alignment() {
        // Default: 32 GiB until the content passes 15.5 GiB.
        assert_eq!(disk_size(None, 0), 32 * GIB);
        assert_eq!(disk_size(None, 3 * GIB), 32 * GIB);
        assert_eq!(disk_size(None, 20 * GIB), 41 * GIB);
        // Rounded up to 1 MiB, from --size and from the formula.
        assert_eq!(disk_size(Some(64 * GIB), 0), 64 * GIB);
        assert_eq!(disk_size(Some(GIB + 1), 0), GIB + MIB);
        assert_eq!(disk_size(Some(MIB - 1), 0), MIB);
        assert_eq!(disk_size(None, 16 * GIB + 1), 33 * GIB + MIB);
        // -N only when the default inode count is less than twice the files.
        assert_eq!(inodes(32 * GIB, 100_000), None);
        assert_eq!(inodes(32 * GIB, 2 * 1024 * 1024), Some(4 * 1024 * 1024));
        // About 800 MiB of metadata on 32 GiB (TECHNICAL, step 6).
        let meta = written(0, 32 * GIB, 32 * GIB / 16384) - LUKS_OFFSET;
        assert_eq!(meta, 768 * MIB);
        // --size 100M with 0.3 GiB of content is refused before the passphrase.
        assert!(written(300 * MIB, 100 * MIB, 20_000) - LUKS_OFFSET > 100 * MIB);
        assert!(written(300 * MIB, 32 * GIB, 32 * GIB / 16384) - LUKS_OFFSET < 32 * GIB);
    }

    #[test]
    fn features_match_mke2fs_conf() {
        let conf = include_str!("../assets/mke2fs.conf");
        for f in FEATURES.split(',') {
            assert!(conf.contains(f), "{f} missing from assets/mke2fs.conf");
        }
        assert_eq!(FEATURES.split(',').count(), 14);
    }
}
