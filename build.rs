// Checks the generated assets before `include_bytes!` embeds them (see src/assets.rs).
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::{env, fs};

/// Files that src/assets.rs embeds from the assets directory.
const EMBEDDED: &[&str] = &[
    "vmlinux",
    "initramfs.cpio.gz",
    "podman/bin/podman",
    "podman/bin/crun",
    "podman/bin/fuse-overlayfs",
    "podman/bin/fusermount3",
    "podman/bin/pasta",
    "podman/bin/pasta.avx2",
    "podman/lib/podman/conmon",
    "podman/lib/podman/catatonit",
    "podman/lib/podman/netavark",
    "podman/lib/podman/aardvark-dns",
    "podman/lib/podman/rootlessport",
    "podman/etc/containers/policy.json",
];

fn fail(msg: String) -> ! {
    eprintln!("error: {msg}; run `cargo xtask assets`");
    std::process::exit(1);
}

fn main() {
    let dir = env::var_os("OHMLATL_ASSETS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/out")
        });
    println!("cargo:rerun-if-env-changed=OHMLATL_ASSETS_DIR");
    let sums_path = dir.join("SHA256SUMS");
    println!("cargo:rerun-if-changed={}", sums_path.display());
    let sums = fs::read_to_string(&sums_path)
        .unwrap_or_else(|e| fail(format!("{}: {e}", sums_path.display())));

    for f in EMBEDDED {
        let want = sums
            .lines()
            .find_map(|l| {
                l.split_once("  ")
                    .filter(|(_, name)| name == f)
                    .map(|(h, _)| h)
            })
            .unwrap_or_else(|| fail(format!("{f} is not in {}", sums_path.display())));
        let path = dir.join(f);
        println!("cargo:rerun-if-changed={}", path.display());
        let data = fs::read(&path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
        if format!("{:x}", Sha256::digest(&data)) != want {
            fail(format!("{}: does not match SHA256SUMS", path.display()));
        }
    }
    println!("cargo:rustc-env=OHMLATL_ASSETS_DIR={}", dir.display());
}
