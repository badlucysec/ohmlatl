mod assets;
mod doctor;
mod home;
mod supervise;
mod util;

use std::path::PathBuf;

const USAGE: &str = "usage:
  ohmlatl init <dir>
  ohmlatl doctor [--build|--run]
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
    let home = take(&mut args, "--home")?.map(PathBuf::from);
    let cmd = if args.is_empty() {
        String::new()
    } else {
        args.remove(0)
    };
    match (cmd.as_str(), args.as_slice()) {
        ("--version" | "-V", []) => {
            println!("ohmlatl {}", env!("CARGO_PKG_VERSION"));
            Ok(true)
        }
        ("init", [dir]) => home::init(&PathBuf::from(dir)).map(|()| true),
        ("doctor", []) => doctor::doctor(home.as_deref(), true, true),
        ("doctor", [f]) if f == "--build" => doctor::doctor(home.as_deref(), true, false),
        ("doctor", [f]) if f == "--run" => doctor::doctor(home.as_deref(), false, true),
        // Internal, P0: the supervisor run by hand (see src/supervise.rs).
        ("supervise", _) => {
            let a = &mut args;
            let image = need(take(a, "--image")?, "--image")?;
            let (name, tag) = image.split_once(':').ok_or("--image needs <name>:<tag>")?;
            let s = supervise::Args {
                uid: need(take_num(a, "--uid")?, "--uid")?,
                gid: need(take_num(a, "--gid")?, "--gid")?,
                home: need(home, "--home")?,
                name: name.to_string(),
                tag: tag.to_string(),
                vcpus: take_num(a, "--vcpus")?.unwrap_or(2),
                memory_mib: take_num(a, "--memory")?.unwrap_or(1024),
                tap: need(take(a, "--tap")?, "--tap")?,
                guest: need(take_num(a, "--guest")?, "--guest")?,
                gw: need(take_num(a, "--gw")?, "--gw")?,
            };
            if !a.is_empty() {
                return Err(format!("unexpected arguments: {}", a.join(" ")));
            }
            supervise::run(s).map(|()| true)
        }
        _ => {
            eprintln!("{USAGE}");
            Ok(false)
        }
    }
}
