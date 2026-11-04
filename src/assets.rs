//! Embedded binaries and their extraction (docs/TECHNICAL.md, "Binários embutidos").

use crate::util::{cache_home, sha256_reader};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

macro_rules! asset {
    ($p:literal) => {
        include_bytes!(concat!(env!("OHMLATL_ASSETS_DIR"), "/", $p))
    };
}

/// (path in the extracted directory, mode, contents)
type File = (&'static str, u32, &'static [u8]);

/// Extracted by `launch` (root) to `/run/ohmlatl/bin/<sha256>/`, with a copy of `ohmlatl` itself.
const RUN: &[File] = &[
    (
        "firecracker",
        0o755,
        include_bytes!("../vendor/firecracker/v1.17.0/firecracker-v1.17.0-x86_64"),
    ),
    (
        "jailer",
        0o755,
        include_bytes!("../vendor/firecracker/v1.17.0/jailer-v1.17.0-x86_64"),
    ),
    ("vmlinux", 0o644, asset!("vmlinux")),
    ("initramfs.cpio.gz", 0o644, asset!("initramfs.cpio.gz")),
];

/// Extracted for the build to `$XDG_CACHE_HOME/ohmlatl/bin/<sha256>/`.
const BUILD: &[File] = &[
    (
        "mke2fs.conf",
        0o644,
        include_bytes!("../assets/mke2fs.conf"),
    ),
    ("bin/podman", 0o755, asset!("podman/bin/podman")),
    ("bin/crun", 0o755, asset!("podman/bin/crun")),
    (
        "bin/fuse-overlayfs",
        0o755,
        asset!("podman/bin/fuse-overlayfs"),
    ),
    ("bin/fusermount3", 0o755, asset!("podman/bin/fusermount3")),
    ("bin/pasta", 0o755, asset!("podman/bin/pasta")),
    ("bin/pasta.avx2", 0o755, asset!("podman/bin/pasta.avx2")),
    (
        "lib/podman/conmon",
        0o755,
        asset!("podman/lib/podman/conmon"),
    ),
    (
        "lib/podman/catatonit",
        0o755,
        asset!("podman/lib/podman/catatonit"),
    ),
    (
        "lib/podman/netavark",
        0o755,
        asset!("podman/lib/podman/netavark"),
    ),
    (
        "lib/podman/aardvark-dns",
        0o755,
        asset!("podman/lib/podman/aardvark-dns"),
    ),
    (
        "lib/podman/rootlessport",
        0o755,
        asset!("podman/lib/podman/rootlessport"),
    ),
    (
        "etc/containers/policy.json",
        0o644,
        asset!("podman/etc/containers/policy.json"),
    ),
];

/// SHA-256 of the running `ohmlatl` binary: different versions never share a directory.
pub fn self_sha256() -> io::Result<&'static str> {
    static SUM: OnceLock<String> = OnceLock::new();
    if let Some(s) = SUM.get() {
        return Ok(s);
    }
    let s = sha256_reader(fs::File::open("/proc/self/exe")?)?;
    Ok(SUM.get_or_init(|| s))
}

/// Writes `files` (plus `extra`) to `<parent>/<sha256>/` through a temporary directory and
/// `rename()`, so a concurrent run never sees a half-written directory. Reuses an existing one.
fn extract(parent: &Path, files: &[File], extra: &[(&str, u32, &Path)]) -> io::Result<PathBuf> {
    let dest = parent.join(self_sha256()?);
    if dest.is_dir() {
        return Ok(dest);
    }
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.{}", self_sha256()?, std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let write = |name: &str, mode: u32, data: &mut dyn io::Read| -> io::Result<()> {
        let path = tmp.join(name);
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir)?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&path)?;
        io::copy(data, &mut f)?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.sync_all()
    };
    fs::create_dir(&tmp)?;
    for (name, mode, data) in files {
        write(name, *mode, &mut &data[..])?;
    }
    for (name, mode, src) in extra {
        write(name, *mode, &mut fs::File::open(src)?)?;
    }
    // Directories created above follow the umask; the layout wants 0755.
    for d in walk_dirs(&tmp)? {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755))?;
    }
    match fs::rename(&tmp, &dest) {
        Ok(()) => Ok(dest),
        // Another run won the race: use its copy.
        Err(_) if dest.is_dir() => {
            fs::remove_dir_all(&tmp)?;
            Ok(dest)
        }
        Err(e) => Err(e),
    }
}

fn walk_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = vec![dir.to_path_buf()];
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            out.extend(walk_dirs(&e.path())?);
        }
    }
    Ok(out)
}

/// `/run/ohmlatl/bin/<sha256>/`: Firecracker, jailer, kernel, initramfs and a copy of `ohmlatl`.
pub fn extract_run(run: &Path) -> io::Result<PathBuf> {
    extract(
        &run.join("bin"),
        RUN,
        &[("ohmlatl", 0o755, Path::new("/proc/self/exe"))],
    )
}

/// `$XDG_CACHE_HOME/ohmlatl/bin/<sha256>/`: the Podman bundle and `mke2fs.conf`.
pub fn extract_build() -> io::Result<PathBuf> {
    extract(&cache_home()?.join("ohmlatl/bin"), BUILD, &[])
}

/// A `podman` command that uses only the embedded bundle and the generated configuration.
pub fn podman() -> io::Result<Command> {
    let b = extract_build()?;
    let cache = cache_home()?.join("ohmlatl");
    let runtime =
        std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let conf = cache.join("conf");
    fs::create_dir_all(&conf)?;
    let b_ = b.display();
    let containers = format!(
        "[containers]\n\
         init_path = \"{b_}/lib/podman/catatonit\"\n\n\
         [engine]\n\
         runtime = \"crun\"\n\
         cgroup_manager = \"cgroupfs\"\n\
         events_logger = \"file\"\n\
         helper_binaries_dir = [\"{b_}/lib/podman\", \"{b_}/bin\"]\n\
         conmon_path = [\"{b_}/lib/podman/conmon\"]\n\n\
         [engine.runtimes]\n\
         crun = [\"{b_}/bin/crun\"]\n"
    );
    let storage = format!(
        "[storage]\n\
         driver = \"overlay\"\n\
         runroot = \"{}\"\n\
         graphroot = \"{}\"\n\n\
         [storage.options.overlay]\n\
         mount_program = \"{b_}/bin/fuse-overlayfs\"\n",
        runtime.join("ohmlatl-podman/run").display(),
        cache.join("podman/storage").display(),
    );
    let registries = "unqualified-search-registries = []\nshort-name-mode = \"enforcing\"\n";
    for (name, text) in [
        ("containers.conf", &containers[..]),
        ("storage.conf", &storage[..]),
        ("registries.conf", registries),
    ] {
        write_if_changed(&conf.join(name), text.as_bytes())?;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![b.join("bin")];
    paths.extend(std::env::split_paths(&path));
    let mut cmd = Command::new(b.join("bin/podman"));
    cmd.env(
        "PATH",
        std::env::join_paths(paths).map_err(io::Error::other)?,
    )
    .env("CONTAINERS_CONF", conf.join("containers.conf"))
    .env("CONTAINERS_STORAGE_CONF", conf.join("storage.conf"))
    .env("CONTAINERS_REGISTRIES_CONF", conf.join("registries.conf"))
    .env(
        "CONTAINERS_POLICY_JSON",
        b.join("etc/containers/policy.json"),
    )
    .env("CONTAINERS_HELPER_BINARY_DIR", b.join("lib/podman"));
    Ok(cmd)
}

fn write_if_changed(path: &Path, data: &[u8]) -> io::Result<()> {
    if fs::read(path).ok().as_deref() == Some(data) {
        return Ok(());
    }
    let tmp = path.with_extension("tmp");
    fs::File::create(&tmp)?.write_all(data)?;
    fs::rename(tmp, path)
}
