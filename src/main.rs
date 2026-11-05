mod assets;
mod build;
mod doctor;
mod home;
mod image;
mod manifest;
mod net;
mod supervise;
mod util;
mod vm;

use std::os::unix::process::CommandExt;
use std::path::PathBuf;

const USAGE: &str = "usage:
  ohmlatl init <dir>
  ohmlatl doctor [--build|--run]
  ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
  ohmlatl podman <args>
  ohmlatl image ls
  ohmlatl image inspect|rm <name:tag>
  ohmlatl image resize <name:tag> <size>
  ohmlatl image passwd <name:tag>
  ohmlatl run <name:tag> [--vcpus n] [--memory n]
  ohmlatl stop <vm>
  ohmlatl ps
  ohmlatl console <vm>
  ohmlatl logs <vm> [-f]
  ohmlatl shell <vm> [-- cmd]
  ohmlatl exec <vm> -- cmd
  ohmlatl cp <vm>:<path> <local>
  ohmlatl cp <local> <vm>:<path>
  ohmlatl --version

Every command accepts --home <dir>.";

fn main() {
    let code = match run(std::env::args().skip(1).collect()) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("ohmlatl: error: {e}");
            1
        }
    };
    std::process::exit(code);
}

/// Value of `--flag <v>`, removed from `args`.
fn take(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if i + 1 >= args.len() {
        return Err(format!("{flag} needs a value"));
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Ok(Some(v))
}

fn take_num<T: std::str::FromStr>(args: &mut Vec<String>, flag: &str) -> Result<Option<T>, String> {
    take(args, flag)?
        .map(|v| v.parse().map_err(|_| format!("{flag}: bad value {v}")))
        .transpose()
}

fn need<T>(v: Option<T>, flag: &str) -> Result<T, String> {
    v.ok_or_else(|| format!("missing {flag}"))
}

fn run(mut args: Vec<String>) -> Result<bool, String> {
    // Everything after `podman` belongs to Podman, --home included.
    let p = if args.first().is_some_and(|a| a == "--home") {
        2
    } else {
        0
    };
    if args.get(p).is_some_and(|a| a == "podman") {
        let mut cmd = assets::podman().map_err(|e| format!("embedded podman: {e}"))?;
        return Err(format!("podman: {}", cmd.args(&args[p + 1..]).exec()));
    }
    // After `--` comes a command for the guest: no flag of ours is taken from it.
    let tail = args
        .iter()
        .position(|a| a == "--")
        .map(|i| args.split_off(i)[1..].to_vec());
    let home = take(&mut args, "--home")?.map(PathBuf::from);
    let cmd = if args.is_empty() {
        String::new()
    } else {
        args.remove(0)
    };
    match (cmd.as_str(), args.as_slice(), tail) {
        ("shell", [vm], cmd) => vm::ssh(vm, &cmd.unwrap_or_default(), true).map(|()| true),
        ("exec", [vm], Some(cmd)) if !cmd.is_empty() => vm::ssh(vm, &cmd, false).map(|()| true),
        (_, _, Some(_)) => {
            eprintln!("{USAGE}");
            Ok(false)
        }
        ("--version" | "-V", [], None) => {
            println!("ohmlatl {}", env!("CARGO_PKG_VERSION"));
            Ok(true)
        }
        ("init", [dir], None) => home::init(&PathBuf::from(dir)).map(|()| true),
        ("doctor", [], None) => doctor::doctor(home.as_deref(), true, true),
        ("doctor", [f], None) if f == "--build" => doctor::doctor(home.as_deref(), true, false),
        ("doctor", [f], None) if f == "--run" => doctor::doctor(home.as_deref(), false, true),
        ("build", _, None) => {
            let a = &mut args;
            let o = build::Opts {
                file: take(a, "-f")?,
                from: take(a, "--from")?,
                tag: take(a, "-t")?,
                size: take(a, "--size")?
                    .map(|s| {
                        manifest::parse_size(&s)
                            .filter(|n| *n > 0)
                            .ok_or(format!("--size: bad value {s}"))
                    })
                    .transpose()?,
            };
            if !a.is_empty() {
                return Err(format!("unexpected arguments: {}", a.join(" ")));
            }
            build::build(home.as_deref(), o).map(|()| true)
        }
        ("image", [c], None) if c == "ls" => image::ls(home.as_deref()).map(|()| true),
        ("image", [c, r], None) if c == "inspect" => {
            image::inspect(home.as_deref(), r).map(|()| true)
        }
        ("image", [c, r], None) if c == "rm" => image::rm(home.as_deref(), r).map(|()| true),
        ("image", [c, r, size], None) if c == "resize" => {
            image::resize(home.as_deref(), r, size).map(|()| true)
        }
        ("image", [c, r], None) if c == "passwd" => {
            image::passwd(home.as_deref(), r).map(|()| true)
        }
        ("run", _, None) => {
            let a = &mut args;
            let (vcpus, memory) = (take(a, "--vcpus")?, take(a, "--memory")?);
            match a.as_slice() {
                [r] => vm::run(home.as_deref(), r, vcpus, memory).map(|()| true),
                _ => Err("usage: ohmlatl run <name:tag> [--vcpus n] [--memory n]".into()),
            }
        }
        ("stop", [vm], None) => vm::stop(vm).map(|()| true),
        ("ps", [], None) => vm::ps().map(|()| true),
        ("console", [vm], None) => vm::console(vm).map(|()| true),
        ("logs", [vm], None) => vm::logs(vm, false).map(|()| true),
        ("logs", [a, b], None) if a == "-f" || b == "-f" => {
            vm::logs(if a == "-f" { b } else { a }, true).map(|()| true)
        }
        ("cp", [src, dst], None) => vm::cp(src, dst).map(|()| true),
        // Internal: `sudo ohmlatl launch` from `run`, then the unit's commands.
        ("launch", _, None) => {
            let a = &mut args;
            let l = vm::Launch {
                uid: need(take_num(a, "--uid")?, "--uid")?,
                gid: need(take_num(a, "--gid")?, "--gid")?,
                home: need(home, "--home")?,
                image: need(take(a, "--image")?, "--image")?,
                vcpus: take(a, "--vcpus")?,
                memory: take(a, "--memory")?,
                pool: take(a, "--pool")?,
            };
            if !a.is_empty() {
                return Err(format!("unexpected arguments: {}", a.join(" ")));
            }
            vm::launch(l).map(|()| true)
        }
        ("supervise", _, None) => {
            let a = &mut args;
            let image = need(take(a, "--image")?, "--image")?;
            let (name, tag) = image.split_once(':').ok_or("--image needs <name>:<tag>")?;
            let s = supervise::Args {
                uid: need(take_num(a, "--uid")?, "--uid")?,
                gid: need(take_num(a, "--gid")?, "--gid")?,
                home: need(home, "--home")?,
                name: name.to_string(),
                tag: tag.to_string(),
                vcpus: take_num(a, "--vcpus")?,
                memory: take(a, "--memory")?,
                pool: take(a, "--pool")?.unwrap_or_else(|| "172.31.0.0/16".into()),
            };
            if !a.is_empty() {
                return Err(format!("unexpected arguments: {}", a.join(" ")));
            }
            supervise::run(s).map(|()| true)
        }
        ("cleanup", [uid, name], None) => {
            let uid: u32 = uid.parse().map_err(|_| format!("cleanup: bad uid {uid}"))?;
            if unsafe { libc::geteuid() } != 0 || !supervise::valid_name(name) {
                return Err("usage: ohmlatl cleanup <uid> <name> (as root)".into());
            }
            supervise::cleanup(uid, name);
            Ok(true)
        }
        _ => {
            eprintln!("{USAGE}");
            Ok(false)
        }
    }
}
