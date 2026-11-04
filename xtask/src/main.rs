//! `cargo xtask assets [--update-lock]`: builds the guest kernel and initramfs (Buildroot)
//! and the Podman bundle (podman-static recipe) in containers, with their exact sources,
//! into `assets/out/`. See docs/TECHNICAL.md, "Geração dos assets".

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const BR_IMAGE: &str = "localhost/ohmlatl-assets-buildroot";
/// Stages built in order; each one is tagged `localhost/ohmlatl-podman-<stage>`.
const STAGES: &[&str] = &[
    "golangbuildbase",
    "rustbase",
    "podman",
    "conmon",
    "netavark",
    "aardvark-dns",
    "passt",
    "fuse-overlayfs",
    "catatonit",
    "crun",
    "podmanbase",
    "rootlesspodmanbase",
    "podmanall",
];
/// Stages that compile an embedded component; their sources go to `out/sources/podman/<stage>/`.
const SOURCE_STAGES: &[&str] = &[
    "podman",
    "conmon",
    "netavark",
    "aardvark-dns",
    "passt",
    "fuse-overlayfs",
    "catatonit",
    "crun",
];
/// (path in the podmanall image, path in the bundle). `cp -L` turns the pasta symlinks into files.
const BUNDLE: &[(&str, &str)] = &[
    ("/usr/local/bin/podman", "bin/podman"),
    ("/usr/local/bin/crun", "bin/crun"),
    ("/usr/local/bin/fuse-overlayfs", "bin/fuse-overlayfs"),
    ("/usr/local/bin/fusermount3", "bin/fusermount3"),
    ("/usr/local/bin/pasta", "bin/pasta"),
    ("/usr/local/bin/pasta.avx2", "bin/pasta.avx2"),
    ("/usr/local/lib/podman/conmon", "lib/podman/conmon"),
    ("/usr/local/lib/podman/catatonit", "lib/podman/catatonit"),
    ("/usr/local/lib/podman/netavark", "lib/podman/netavark"),
    (
        "/usr/local/lib/podman/aardvark-dns",
        "lib/podman/aardvark-dns",
    ),
    (
        "/usr/local/lib/podman/rootlessport",
        "lib/podman/rootlessport",
    ),
    ("/etc/containers/policy.json", "etc/containers/policy.json"),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let update = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["assets"] => false,
        ["assets", "--update-lock"] => true,
        _ => {
            eprintln!("usage: cargo xtask assets [--update-lock]");
            std::process::exit(2);
        }
    };
    if let Err(e) = assets(update) {
        eprintln!("xtask: {e}");
        std::process::exit(1);
    }
}

struct Paths {
    assets: PathBuf,
    out: PathBuf,
    cache: PathBuf,
}

fn assets(update: bool) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let cache = match std::env::var_os("XDG_CACHE_HOME") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var("HOME")?).join(".cache"),
    }
    .join("ohmlatl-assets");
    let p = Paths {
        assets: root.join("assets"),
        out: root.join("assets/out"),
        cache,
    };
    for d in ["dl", "ccache", "build", "src", "cargo-about", "aports"] {
        fs::create_dir_all(p.cache.join(d))?;
    }
    fs::create_dir_all(&p.out)?;
    kernel(&p, &root)?;
    podman_bundle(&p, update)?;
    sha256sums(&p.out)?;
    println!("xtask: assets ready in {}", p.out.display());
    Ok(())
}

// ---------- kernel and initramfs ----------

fn kernel(p: &Paths, root: &Path) -> Result<()> {
    let lock = fs::read_to_string(p.assets.join("buildroot.lock"))?;
    let field = |k: &str| -> Result<String> {
        lock.lines()
            .find_map(|l| {
                l.strip_prefix(k)
                    .and_then(|v| v.strip_prefix(' '))
                    .map(str::to_string)
            })
            .ok_or_else(|| format!("buildroot.lock: missing {k}").into())
    };
    let (version, url, sum) = (field("version")?, field("url")?, field("sha256")?);
    let tarball = p.cache.join("dl").join(url.rsplit('/').next().unwrap());
    if !tarball.exists() {
        let part = tarball.with_extension("part");
        run(Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&part)
            .arg(&url))?;
        fs::rename(&part, &tarball)?;
    }
    if sha256_file(&tarball)? != sum {
        return Err(format!(
            "{}: sha256 does not match buildroot.lock",
            tarball.display()
        )
        .into());
    }
    let src = p.cache.join("src").join(format!("buildroot-{version}"));
    if !src.exists() {
        run(Command::new("tar")
            .arg("-xJf")
            .arg(&tarball)
            .arg("-C")
            .arg(p.cache.join("src")))?;
    }

    println!("xtask: building {BR_IMAGE}");
    run(podman()
        .args(["build", "-q", "-t", BR_IMAGE, "-f"])
        .arg(p.assets.join("Containerfile"))
        .arg(&p.assets))?;
    println!("xtask: Buildroot {version} (the first build takes 40 to 60 minutes)");
    let mk = "make O=/cache/build BR2_EXTERNAL=/work/assets/br2-external";
    run(podman()
        .args(["run", "--rm", "--userns=keep-id", "-e", "HOME=/tmp"])
        .args([
            "-e",
            "BR2_DL_DIR=/cache/dl",
            "-e",
            "BR2_CCACHE_DIR=/cache/ccache",
        ])
        .arg("-v")
        .arg(format!("{}:/work:ro", root.display()))
        .arg("-v")
        .arg(format!("{}:/cache", p.cache.display()))
        .arg("-w")
        .arg(format!("/cache/src/buildroot-{version}"))
        .args([BR_IMAGE, "sh", "-euc"])
        .arg(format!(
            "{mk} ohmlatl_defconfig\n\
             /work/assets/br2-external/board/ohmlatl/check-brconfig.sh /cache/build/.config\n\
             {mk}\n\
             {mk} legal-info"
        )))?;

    let images = p.cache.join("build/images");
    fs::copy(images.join("vmlinux"), p.out.join("vmlinux"))?;
    fs::copy(
        images.join("rootfs.cpio.gz"),
        p.out.join("initramfs.cpio.gz"),
    )?;
    replace_dir(&p.cache.join("build/legal-info"), &p.out.join("legal-info"))?;
    Ok(())
}

// ---------- Podman bundle ----------

/// `podman.lock`, kept as ordered lines so `--update-lock` rewrites it in place.
struct Lock {
    recipe: (String, String, String),
    /// build arg -> (tag reference, digest)
    images: Vec<(String, String, String)>,
    apk: BTreeMap<String, Vec<String>>,
    /// stage -> file -> sha256
    sources: BTreeMap<String, BTreeMap<String, String>>,
}

impl Lock {
    fn read(path: &Path) -> Result<Lock> {
        let mut l = Lock {
            recipe: Default::default(),
            images: vec![],
            apk: BTreeMap::new(),
            sources: BTreeMap::new(),
        };
        for line in fs::read_to_string(path)?.lines() {
            let w: Vec<&str> = line.split_whitespace().collect();
            match w.as_slice() {
                [] => {}
                [c, ..] if c.starts_with('#') => {}
                ["recipe", url, tag, commit] => {
                    l.recipe = (url.to_string(), tag.to_string(), commit.to_string())
                }
                ["image", arg, r, d] => {
                    l.images
                        .push((arg.to_string(), r.to_string(), d.to_string()))
                }
                ["apk", stage, pkgs @ ..] => {
                    l.apk.insert(
                        stage.to_string(),
                        pkgs.iter().map(|s| s.to_string()).collect(),
                    );
                }
                ["source", stage, file, sum] => {
                    l.sources
                        .entry(stage.to_string())
                        .or_default()
                        .insert(file.to_string(), sum.to_string());
                }
                _ => return Err(format!("podman.lock: bad line: {line}").into()),
            }
        }
        Ok(l)
    }

    fn write(&self, path: &Path) -> Result<()> {
        let mut s = String::from(
            "# Pins for the podman-static recipe, read by `cargo xtask assets`.\n\
             # Regenerate with `cargo xtask assets --update-lock` and review the diff.\n",
        );
        let (url, tag, commit) = &self.recipe;
        s += &format!("recipe {url} {tag} {commit}\n");
        for (arg, r, d) in &self.images {
            s += &format!("image {arg} {r} {d}\n");
        }
        for stage in STAGES {
            if let Some(pkgs) = self.apk.get(*stage) {
                s += &format!("apk {stage} {}\n", pkgs.join(" "));
            }
        }
        for (stage, files) in &self.sources {
            for (f, sum) in files {
                s += &format!("source {stage} {f} {sum}\n");
            }
        }
        fs::write(path, s)?;
        Ok(())
    }
}

fn podman_bundle(p: &Paths, update: bool) -> Result<()> {
    let lock_path = p.assets.join("podman/podman.lock");
    let mut lock = Lock::read(&lock_path)?;
    let (url, tag, commit) = lock.recipe.clone();

    let recipe = p.cache.join("src").join(format!("podman-static-{commit}"));
    if !recipe.exists() {
        run(Command::new("git")
            .args([
                "clone",
                "-q",
                "-c",
                "advice.detachedHead=false",
                "--depth=1",
                "--branch",
                &tag,
                &url,
            ])
            .arg(&recipe))?;
    }
    let head = output(
        Command::new("git")
            .arg("-C")
            .arg(&recipe)
            .args(["rev-parse", "HEAD"]),
    )?;
    if head.trim() != commit {
        return Err(format!(
            "podman-static {tag} is {}, podman.lock says {commit}",
            head.trim()
        )
        .into());
    }
    let ctx = p.cache.join("podman-ctx");
    let _ = fs::remove_dir_all(&ctx);
    run(Command::new("cp").arg("-a").arg(&recipe).arg(&ctx))?;
    run(Command::new("patch")
        .arg("-s")
        .arg("-d")
        .arg(&ctx)
        .arg("-p1")
        .arg("-i")
        .arg(p.assets.join("podman/pin-bases.patch")))?;

    // Every stage gets the same build args, so the layer cache holds across stages;
    // with --update-lock the new `apk info -v` lists only replace the lock at the end.
    let mut new_apk = BTreeMap::new();
    for stage in STAGES {
        println!("xtask: podman-static stage {stage}");
        let mut cmd = podman();
        cmd.args([
            "build",
            "--target",
            stage,
            "-t",
            &format!("localhost/ohmlatl-podman-{stage}"),
        ]);
        for (arg, r, d) in &lock.images {
            let name = r.rsplit_once(':').map_or(r.as_str(), |(n, _)| n);
            cmd.arg("--build-arg").arg(format!("{arg}={name}@{d}"));
        }
        for (s, pkgs) in &lock.apk {
            let pkgs: Vec<&str> = if update {
                pkgs.iter().map(|p| p.split('=').next().unwrap()).collect()
            } else {
                if let Some(bad) = pkgs.iter().find(|p| !p.contains('=')) {
                    return Err(format!("podman.lock: {s}: {bad} has no version; run `cargo xtask assets --update-lock`").into());
                }
                pkgs.iter().map(String::as_str).collect()
            };
            cmd.arg("--build-arg")
                .arg(format!("APK_{}={}", s.replace('-', "_"), pkgs.join(" ")));
        }
        cmd.arg(&ctx);
        run(&mut cmd).map_err(|e| {
            format!("{e}\nxtask: if a pinned apk version is gone from the Alpine mirror, run `cargo xtask assets --update-lock`")
        })?;
        if update && lock.apk.contains_key(*stage) {
            let info = output(podman().args([
                "run",
                "--rm",
                &format!("localhost/ohmlatl-podman-{stage}"),
                "apk",
                "info",
                "-v",
            ]))?;
            let mut pkgs: Vec<String> = info.lines().map(apk_pin).collect::<Result<_>>()?;
            pkgs.sort();
            new_apk.insert(stage.to_string(), pkgs);
        }
    }
    if update {
        lock.apk = new_apk;
    }

    let sources = p.out.join("sources/podman");
    fs::create_dir_all(&sources)?;
    for stage in SOURCE_STAGES {
        println!("xtask: capturing sources of {stage}");
        let dir = sources.join(stage);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        run(podman()
            .args(["run", "--rm"])
            .arg("-v")
            .arg(format!("{}:/out", dir.display()))
            .arg("-v")
            .arg(format!("{}:/aports", p.cache.join("aports").display()))
            .arg("-v")
            .arg(format!(
                "{}:/opt/cargo-about",
                p.cache.join("cargo-about").display()
            ))
            .arg("-v")
            .arg(format!(
                "{}:/capture-sources.sh:ro",
                p.assets.join("podman/capture-sources.sh").display()
            ))
            .arg(format!("localhost/ohmlatl-podman-{stage}"))
            .args(["sh", "/capture-sources.sh", stage, "/out", "/aports"]))?;
        let got: BTreeMap<String, String> = fs::read_to_string(dir.join("SOURCES"))?
            .lines()
            .filter_map(|l| {
                l.split_once(' ')
                    .map(|(f, s)| (f.to_string(), s.to_string()))
            })
            .collect();
        if update {
            lock.sources.insert(stage.to_string(), got);
        } else if lock.sources.get(*stage) != Some(&got) {
            return Err(format!(
                "{stage}: sources differ from podman.lock (see {})",
                dir.join("SOURCES").display()
            )
            .into());
        }
    }
    // The aports trees of the static libraries, shared by all stages.
    replace_dir(&p.cache.join("aports"), &sources.join("aports"))?;
    // The recipe itself and the scripts that pin and capture it.
    let recipe_tar = sources.join(format!("podman-static-{tag}.tar"));
    run(Command::new("git")
        .arg("-C")
        .arg(&recipe)
        .args([
            "archive",
            "--format=tar",
            &format!("--prefix=podman-static-{tag}/"),
            "-o",
        ])
        .arg(&recipe_tar)
        .arg("HEAD"))?;
    for f in ["pin-bases.patch", "capture-sources.sh"] {
        fs::copy(p.assets.join("podman").join(f), sources.join(f))?;
    }

    println!("xtask: assembling the bundle");
    let bundle = p.out.join("podman");
    let _ = fs::remove_dir_all(&bundle);
    fs::create_dir_all(&bundle)?;
    let script: String = BUNDLE
        .iter()
        .map(|(from, to)| {
            format!("install -D -m 0755 /dev/null /out/{to} && cp -L {from} /out/{to}\n")
        })
        .collect();
    run(podman()
        .args(["run", "--rm", "-v"])
        .arg(format!("{}:/out", bundle.display()))
        .args(["localhost/ohmlatl-podman-podmanall", "sh", "-euc", &script]))?;
    fs::set_permissions(
        bundle.join("etc/containers/policy.json"),
        std::os::unix::fs::PermissionsExt::from_mode(0o644),
    )?;

    if update {
        lock.write(&lock_path)?;
        println!("xtask: rewrote {}", lock_path.display());
    }
    fs::copy(&lock_path, sources.join("podman.lock"))?;
    Ok(())
}

/// `musl-1.2.5-r10` -> `musl=1.2.5-r10`
fn apk_pin(s: &str) -> Result<String> {
    let bad = || format!("unexpected `apk info -v` line: {s}");
    let (rest, rel) = s.rsplit_once('-').ok_or_else(bad)?;
    let (name, ver) = rest.rsplit_once('-').ok_or_else(bad)?;
    if !rel.starts_with('r') {
        return Err(bad().into());
    }
    Ok(format!("{name}={ver}-{rel}"))
}

// ---------- helpers ----------

fn podman() -> Command {
    Command::new("podman")
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !status.success() {
        return Err(format!(
            "{:?} {:?} failed: {status}",
            cmd.get_program(),
            cmd.get_args().collect::<Vec<_>>()
        )
        .into());
    }
    Ok(())
}

fn output(cmd: &mut Command) -> Result<String> {
    let o = cmd.stderr(Stdio::inherit()).output()?;
    if !o.status.success() {
        return Err(format!(
            "{:?} {:?} failed: {}",
            cmd.get_program(),
            cmd.get_args().collect::<Vec<_>>(),
            o.status
        )
        .into());
    }
    Ok(String::from_utf8(o.stdout)?)
}

fn replace_dir(from: &Path, to: &Path) -> Result<()> {
    let _ = fs::remove_dir_all(to);
    run(Command::new("cp").arg("-a").arg(from).arg(to))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// `SHA256SUMS` of every file in `out/`, sorted, in `sha256sum` format.
fn sha256sums(out: &Path) -> Result<()> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let t = e.file_type()?;
            if t.is_dir() {
                walk(&e.path(), files)?;
            } else if t.is_file() {
                files.push(e.path());
            }
        }
        Ok(())
    }
    let mut files = vec![];
    walk(out, &mut files)?;
    files.sort();
    let mut s = String::new();
    for f in files {
        let rel = f.strip_prefix(out)?;
        if rel == Path::new("SHA256SUMS") {
            continue;
        }
        s += &format!("{}  {}\n", sha256_file(&f)?, rel.display());
    }
    fs::write(out.join("SHA256SUMS"), s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn apk_pin() {
        assert_eq!(super::apk_pin("musl-1.2.5-r10").unwrap(), "musl=1.2.5-r10");
        assert_eq!(
            super::apk_pin("libseccomp-static-2.6.0-r0").unwrap(),
            "libseccomp-static=2.6.0-r0"
        );
        assert!(super::apk_pin("nonsense").is_err());
    }
}
