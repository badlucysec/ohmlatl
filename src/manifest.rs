//! `ohmlatl.yaml` and `manifest.yaml` (docs/TECHNICAL.md, "Manifest").
//!
//! The parser reads the YAML subset the manifest needs: block mappings, block lists, plain or
//! quoted scalars, `[]` and comments. Anything else is an error, like an unknown field.

use crate::supervise::{valid_name, valid_tag};
use std::collections::HashSet;

#[derive(Debug, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub containerfile: Option<String>,
    pub tag: Option<String>,
    pub user: User,
    pub vcpus: u32,
    /// As written (`4GiB`); checked with `parse_size`.
    pub memory: String,
    pub internet: bool,
    pub forward: Vec<Forward>,
}

#[derive(Debug, PartialEq)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, PartialEq)]
pub struct Forward {
    pub port: u16,
    pub to: String,
}

#[derive(Debug)]
enum Node {
    Scalar(String),
    Map(Vec<(String, Node)>),
    List(Vec<Node>),
}

/// Parses and validates a manifest.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let mut lines = vec![];
    for (n, raw) in text.lines().enumerate() {
        let body = raw.trim_start_matches(' ');
        if body.starts_with('\t') {
            return Err(format!("line {}: tab in indentation", n + 1));
        }
        let body = strip_comment(body).trim_end();
        if !body.is_empty() {
            lines.push((
                n + 1,
                raw.len() - raw.trim_start_matches(' ').len(),
                body.to_string(),
            ));
        }
    }
    if lines.is_empty() {
        return Err("empty manifest".into());
    }
    let mut i = 0;
    let indent = lines[0].1;
    let root = block(&mut lines, &mut i, indent)?;
    if let Some((n, ..)) = lines.get(i) {
        return Err(format!("line {n}: bad indentation"));
    }
    manifest(&root)
}

/// Text before a `#` that starts a comment (at the start or after a space, outside quotes).
fn strip_comment(s: &str) -> &str {
    let mut quote = None;
    let mut prev = ' ';
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && prev == ' ' => return &s[..i],
            None => {}
        }
        prev = c;
    }
    s
}

type Line = (usize, usize, String);

fn block(lines: &mut [Line], i: &mut usize, indent: usize) -> Result<Node, String> {
    if is_item(&lines[*i].2) {
        list(lines, i, indent)
    } else {
        map(lines, i, indent)
    }
}

fn is_item(s: &str) -> bool {
    s == "-" || s.starts_with("- ")
}

fn map(lines: &mut [Line], i: &mut usize, indent: usize) -> Result<Node, String> {
    let mut out: Vec<(String, Node)> = vec![];
    while let Some((n, ind, body)) = lines.get(*i).cloned() {
        if ind < indent {
            break;
        }
        if ind > indent || is_item(&body) {
            return Err(format!("line {n}: bad indentation"));
        }
        let (key, value) = match body.split_once(": ") {
            Some((k, v)) => (k, v.trim()),
            None => (
                body.strip_suffix(':')
                    .ok_or(format!("line {n}: expected `key: value`"))?,
                "",
            ),
        };
        if key.is_empty() || !key.bytes().all(|c| c.is_ascii_lowercase() || c == b'_') {
            return Err(format!("line {n}: bad key {key:?}"));
        }
        if out.iter().any(|(k, _)| k == key) {
            return Err(format!("line {n}: duplicate key {key}"));
        }
        *i += 1;
        let node = match lines.get(*i) {
            // `key:` with a nested block, or a list at the same indentation.
            Some((_, ind2, b))
                if value.is_empty() && (*ind2 > indent || (*ind2 == indent && is_item(b))) =>
            {
                block(lines, i, *ind2)?
            }
            _ if value == "[]" => Node::List(vec![]),
            _ => Node::Scalar(scalar(value, n)?),
        };
        out.push((key.to_string(), node));
    }
    Ok(Node::Map(out))
}

fn list(lines: &mut [Line], i: &mut usize, indent: usize) -> Result<Node, String> {
    let mut out = vec![];
    while let Some((n, ind, body)) = lines.get(*i).cloned() {
        if ind < indent || (ind == indent && !is_item(&body)) {
            break;
        }
        if ind > indent {
            return Err(format!("line {n}: bad indentation"));
        }
        let rest = body[1..].trim_start();
        if rest.is_empty() {
            *i += 1;
            match lines.get(*i) {
                Some((_, ind2, _)) if *ind2 > indent => out.push(block(lines, i, *ind2)?),
                _ => return Err(format!("line {n}: empty list item")),
            }
        } else if rest.contains(": ") || rest.ends_with(':') {
            // `- key: value` starts a mapping at the column of `key`.
            let col = indent + body.len() - rest.len();
            lines[*i] = (n, col, rest.to_string());
            out.push(map(lines, i, col)?);
        } else {
            out.push(Node::Scalar(scalar(rest, n)?));
            *i += 1;
        }
    }
    Ok(Node::List(out))
}

fn scalar(v: &str, n: usize) -> Result<String, String> {
    for q in ['"', '\''] {
        if let Some(s) = v.strip_prefix(q) {
            return match s.strip_suffix(q) {
                Some(s) if !s.contains(q) => Ok(s.to_string()),
                _ => Err(format!("line {n}: bad quoted value {v}")),
            };
        }
    }
    if v.starts_with(['{', '[', '&', '*', '!', '|', '>', '%', '@', '`']) {
        return Err(format!("line {n}: unsupported YAML value {v}"));
    }
    Ok(v.to_string())
}

/// The fields of a mapping at `at`, refusing keys not in `allowed`.
fn fields<'a>(n: &'a Node, at: &str, allowed: &[&str]) -> Result<Vec<(&'a str, &'a Node)>, String> {
    let Node::Map(m) = n else {
        return Err(format!("{at}: expected a mapping"));
    };
    let mut out = vec![];
    for (k, v) in m {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("unknown field {at}{k}"));
        }
        out.push((k.as_str(), v));
    }
    Ok(out)
}

fn text<'a>(n: &'a Node, at: &str) -> Result<&'a str, String> {
    match n {
        Node::Scalar(s) => Ok(s),
        _ => Err(format!("{at}: expected a value")),
    }
}

fn num<T: std::str::FromStr + PartialOrd + std::fmt::Display>(
    n: &Node,
    at: &str,
    min: T,
    max: T,
) -> Result<T, String> {
    let s = text(n, at)?;
    match s.parse::<T>() {
        Ok(v) if v >= min && v <= max => Ok(v),
        _ => Err(format!("{at}: {s} is not a number between {min} and {max}")),
    }
}

fn manifest(root: &Node) -> Result<Manifest, String> {
    let (mut format, mut name, mut build, mut user, mut vm) = (None, None, None, None, None);
    for (k, v) in fields(root, "", &["format", "name", "build", "user", "vm"])? {
        match k {
            "format" => format = Some(text(v, "format")?),
            "name" => name = Some(text(v, "name")?),
            "build" => build = Some(v),
            "user" => user = Some(v),
            _ => vm = Some(v),
        }
    }
    match format {
        Some("1") => {}
        Some(f) => return Err(format!("format: unknown version {f}")),
        None => return Err("missing field format".into()),
    }
    let name = name.ok_or("missing field name")?.to_string();
    if !valid_name(&name) {
        return Err(format!(
            "name: invalid {name:?} (want ^[a-z][a-z0-9-]{{0,30}}$)"
        ));
    }

    let (mut containerfile, mut tag) = (None, None);
    if let Some(b) = build {
        for (k, v) in fields(b, "build.", &["containerfile", "tag"])? {
            let s = text(v, &format!("build.{k}"))?.to_string();
            match k {
                "containerfile" if s.is_empty() || s.contains('"') => {
                    return Err(format!("build.containerfile: invalid {s:?}"));
                }
                "containerfile" => containerfile = Some(s),
                _ if !valid_tag(&s) => {
                    return Err(format!(
                        "build.tag: invalid {s:?} (want ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$)"
                    ));
                }
                _ => tag = Some(s),
            }
        }
    }

    let (mut uname, mut uid, mut gid) = (None, None, None);
    for (k, v) in fields(
        user.ok_or("missing field user")?,
        "user.",
        &["name", "uid", "gid"],
    )? {
        match k {
            "name" => uname = Some(text(v, "user.name")?.to_string()),
            "uid" => uid = Some(num(v, "user.uid", 1u32, 65534)?),
            _ => gid = Some(num(v, "user.gid", 1u32, 65534)?),
        }
    }
    let uname = uname.ok_or("missing field user.name")?;
    // It ends up in the serial-getty unit: only plain user names.
    let b = uname.as_bytes();
    if !(1..=32).contains(&b.len())
        || !(b[0].is_ascii_lowercase() || b[0] == b'_')
        || !b
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(c))
    {
        return Err(format!("user.name: invalid {uname:?}"));
    }
    let user = User {
        name: uname,
        uid: uid.ok_or("missing field user.uid")?,
        gid: gid.ok_or("missing field user.gid")?,
    };

    let (mut vcpus, mut memory, mut network) = (None, None, None);
    for (k, v) in fields(
        vm.ok_or("missing field vm")?,
        "vm.",
        &["vcpus", "memory", "network"],
    )? {
        match k {
            "vcpus" => vcpus = Some(num(v, "vm.vcpus", 1u32, 32)?),
            "memory" => memory = Some(text(v, "vm.memory")?.to_string()),
            _ => network = Some(v),
        }
    }
    let memory = memory.ok_or("missing field vm.memory")?;
    match parse_size(&memory) {
        Some(m) if m >= 512 << 20 => {}
        _ => {
            return Err(format!(
                "vm.memory: {memory} (want a size of at least 512MiB)"
            ));
        }
    }

    let (mut internet, mut forward) = (true, vec![]);
    if let Some(n) = network {
        for (k, v) in fields(n, "vm.network.", &["internet", "forward"])? {
            if k == "internet" {
                internet = match text(v, "vm.network.internet")? {
                    "true" => true,
                    "false" => false,
                    s => return Err(format!("vm.network.internet: {s} (want true or false)")),
                };
                continue;
            }
            let Node::List(items) = v else {
                return Err("vm.network.forward: expected a list".into());
            };
            for (j, it) in items.iter().enumerate() {
                let at = format!("vm.network.forward[{j}]");
                let (mut port, mut to) = (None, None);
                for (k, v) in fields(it, &format!("{at}."), &["port", "to"])? {
                    match k {
                        "port" => port = Some(num(v, &format!("{at}.port"), 1u16, 65535)?),
                        _ => to = Some(text(v, &format!("{at}.to"))?.to_string()),
                    }
                }
                let port = port.ok_or(format!("missing field {at}.port"))?;
                let to = to.ok_or(format!("missing field {at}.to"))?;
                if !valid_target(&to) {
                    return Err(format!(
                        "{at}.to: {to} (want host:<port> or vm/<name>:<port>)"
                    ));
                }
                forward.push(Forward { port, to });
            }
        }
    }
    let mut seen = HashSet::new();
    for f in &forward {
        if !seen.insert(f.port) {
            return Err(format!("vm.network.forward: port {} repeated", f.port));
        }
        if f.port == 53 && internet {
            return Err("vm.network.forward: port 53 is the DNS relay with internet: true".into());
        }
    }

    Ok(Manifest {
        name,
        containerfile,
        tag,
        user,
        vcpus: vcpus.ok_or("missing field vm.vcpus")?,
        memory,
        internet,
        forward,
    })
}

/// `host:<port>` or `vm/<name>:<port>`.
fn valid_target(to: &str) -> bool {
    let Some((dest, port)) = to.rsplit_once(':') else {
        return false;
    };
    let port_ok = port.parse::<u16>().is_ok_and(|p| p >= 1) && !port.starts_with('0');
    port_ok && (dest == "host" || dest.strip_prefix("vm/").is_some_and(valid_name))
}

impl Manifest {
    /// The manifest as written to `images/<name>/<tag>/manifest.yaml`.
    pub fn to_yaml(&self) -> String {
        let tag = self.tag.as_deref().unwrap_or("latest");
        let mut s = format!("format: 1\nname: {}\n\nbuild:\n", self.name);
        if let Some(c) = &self.containerfile {
            s += &format!("  containerfile: \"{c}\"\n");
        }
        s += &format!(
            "  tag: {tag}\n\nuser:\n  name: {}\n  uid: {}\n  gid: {}\n\nvm:\n  vcpus: {}\n  memory: {}\n  network:\n    internet: {}\n",
            self.user.name, self.user.uid, self.user.gid, self.vcpus, self.memory, self.internet
        );
        if !self.forward.is_empty() {
            s += "    forward:\n";
            for f in &self.forward {
                s += &format!("      - port: {}\n        to: {}\n", f.port, f.to);
            }
        }
        s
    }
}

/// `64G`, `4GiB`, `512M`, `1T` or plain bytes; binary units.
pub fn parse_size(s: &str) -> Option<u64> {
    let digits = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n: u64 = s[..digits].parse().ok()?;
    let shift = match &s[digits..] {
        "" | "B" => 0,
        "K" | "KiB" => 10,
        "M" | "MiB" => 20,
        "G" | "GiB" => 30,
        "T" | "TiB" => 40,
        _ => return None,
    };
    n.checked_mul(1 << shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "format: 1
name: kali-base

build:
  containerfile: Containerfile
  tag: 2026-11-02

user:
  name: dev
  uid: 1000
  gid: 1000

vm:
  vcpus: 2
  memory: 4GiB   # comment
  network:
    internet: true
    forward:
      - port: 11434
        to: host:11434
      - port: 8080
        to: vm/mcp:8080
";

    fn with(from: &str, to: &str) -> Result<Manifest, String> {
        assert!(DOC.contains(from), "{from}");
        parse(&DOC.replace(from, to))
    }

    #[test]
    fn full_example() {
        let m = parse(DOC).unwrap();
        assert_eq!(m.name, "kali-base");
        assert_eq!(m.containerfile.as_deref(), Some("Containerfile"));
        assert_eq!(m.tag.as_deref(), Some("2026-11-02"));
        assert_eq!(
            m.user,
            User {
                name: "dev".into(),
                uid: 1000,
                gid: 1000
            }
        );
        assert_eq!((m.vcpus, m.memory.as_str(), m.internet), (2, "4GiB", true));
        assert_eq!(
            m.forward,
            vec![
                Forward {
                    port: 11434,
                    to: "host:11434".into()
                },
                Forward {
                    port: 8080,
                    to: "vm/mcp:8080".into()
                }
            ]
        );
        // The copy in the image round-trips, with the final tag.
        let copy = parse(
            &Manifest {
                tag: Some("v2".into()),
                ..m
            }
            .to_yaml(),
        )
        .unwrap();
        assert_eq!(copy.tag.as_deref(), Some("v2"));
        assert_eq!(copy.forward.len(), 2);
    }

    #[test]
    fn minimal_and_defaults() {
        let m = parse(
            "format: 1\nname: a\nuser:\n  name: dev\n  uid: 1\n  gid: 65534\nvm:\n  vcpus: 1\n  memory: 512MiB\n",
        )
        .unwrap();
        assert!(m.internet && m.forward.is_empty() && m.tag.is_none() && m.containerfile.is_none());
        // Lists at the key's indentation, quotes and `[]`.
        let m = with("    forward:\n      - port: 11434\n        to: host:11434\n      - port: 8080\n        to: vm/mcp:8080\n",
            "    forward:\n    - port: 22\n      to: \"host:2222\"\n").unwrap();
        assert_eq!(
            m.forward,
            vec![Forward {
                port: 22,
                to: "host:2222".into()
            }]
        );
        let m = with("    forward:\n      - port: 11434\n        to: host:11434\n      - port: 8080\n        to: vm/mcp:8080\n",
            "    forward: []\n").unwrap();
        assert!(m.forward.is_empty());
    }

    #[test]
    fn structure_errors() {
        assert!(
            with("format: 1", "format: 2")
                .unwrap_err()
                .contains("unknown version")
        );
        assert!(
            with("format: 1\n", "")
                .unwrap_err()
                .contains("missing field format")
        );
        assert!(
            with("  gid: 1000\n", "  gid: 1000\n  shell: zsh\n")
                .unwrap_err()
                .contains("unknown field user.shell")
        );
        assert!(
            with("name: kali-base\n", "name: kali-base\nextra: 1\n")
                .unwrap_err()
                .contains("unknown field extra")
        );
        assert!(
            with("  uid: 1000\n", "  uid: 1000\n  uid: 1000\n")
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(
            with("  uid: 1000\n", "\tuid: 1000\n")
                .unwrap_err()
                .contains("tab")
        );
        assert!(
            with("  uid: 1000\n", "   uid: 1000\n")
                .unwrap_err()
                .contains("indentation")
        );
        assert!(
            with("  vcpus: 2", "  vcpus: {a: 1}")
                .unwrap_err()
                .contains("unsupported")
        );
        assert!(parse("").is_err());
    }

    #[test]
    fn names_and_tags() {
        assert!(
            with("name: kali-base", "name: Kali")
                .unwrap_err()
                .contains("name: invalid")
        );
        assert!(with("name: kali-base", "name: 1kali").is_err());
        assert!(with("name: kali-base", &format!("name: {}", "a".repeat(32))).is_err());
        assert!(
            with("tag: 2026-11-02", "tag: ../x")
                .unwrap_err()
                .contains("build.tag")
        );
        assert!(with("tag: 2026-11-02", "tag: .x").is_err());
        assert!(with("tag: 2026-11-02", "tag: v1.0_rc-2").is_ok());
        assert!(
            with("  name: dev", "  name: \"dev %I\"")
                .unwrap_err()
                .contains("user.name")
        );
        assert!(with("  name: dev", "  name: Dev").is_err());
    }

    #[test]
    fn uid_gid_vm() {
        assert!(
            with("uid: 1000", "uid: 0")
                .unwrap_err()
                .contains("user.uid")
        );
        assert!(with("uid: 1000", "uid: 65535").is_err());
        assert!(
            with("gid: 1000", "gid: -1")
                .unwrap_err()
                .contains("user.gid")
        );
        assert!(with("uid: 1000", "uid: 65534").is_ok());
        assert!(
            with("vcpus: 2", "vcpus: 33")
                .unwrap_err()
                .contains("vm.vcpus")
        );
        assert!(with("vcpus: 2", "vcpus: 0").is_err());
        assert!(
            with("memory: 4GiB", "memory: 511MiB")
                .unwrap_err()
                .contains("vm.memory")
        );
        assert!(with("memory: 4GiB", "memory: 4 GiB").is_err());
        assert!(
            with("internet: true", "internet: yes")
                .unwrap_err()
                .contains("internet")
        );
    }

    #[test]
    fn forwards() {
        let e = |to: &str| with("to: host:11434", &format!("to: {to}"));
        assert!(e("host:0").is_err() && e("host:65536").is_err() && e("host:011").is_err());
        assert!(
            e("lan:80").is_err()
                && e("vm/Bad:80").is_err()
                && e("vm/:80").is_err()
                && e("host").is_err()
        );
        assert!(e("vm/mcp-2:65535").is_ok());
        assert!(
            with("port: 8080", "port: 11434")
                .unwrap_err()
                .contains("repeated")
        );
        assert!(with("port: 8080", "port: 0").is_err());
        assert!(with("port: 8080", "port: 65536").is_err());
        assert!(with("port: 8080", "port: 53").unwrap_err().contains("53"));
        let off = DOC
            .replace("internet: true", "internet: false")
            .replace("port: 8080", "port: 53");
        assert!(parse(&off).is_ok());
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("64G"), Some(64 << 30));
        assert_eq!(parse_size("4GiB"), Some(4 << 30));
        assert_eq!(parse_size("512M"), Some(512 << 20));
        assert_eq!(parse_size("1T"), Some(1 << 40));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("1.5G"), None);
        assert_eq!(parse_size("G"), None);
        assert_eq!(parse_size("4gb"), None);
        assert_eq!(parse_size("99999999999T"), None);
    }
}
