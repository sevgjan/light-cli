//! Token storage: `$LIGHT_TOKEN`, else ~/.config/lpt/credentials.json (0600).

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

fn path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) => PathBuf::from(x),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(base.join("lpt").join("credentials.json"))
}

pub fn load() -> Option<(String, Option<String>)> {
    if let Ok(t) = std::env::var("LIGHT_TOKEN") {
        return Some((t, None));
    }
    let p = path().ok()?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
    restrict(&p, 0o600); // repair a file that someone made group/world-readable
    Some((v["token"].as_str()?.to_string(), v["email"].as_str().map(String::from)))
}

pub fn save(token: &str, email: &str) -> Result<()> {
    let p = path()?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    restrict(p.parent().unwrap(), 0o700);
    write_private(&p, &json!({ "token": token, "email": email }).to_string())
}

#[cfg(unix)]
fn write_private(p: &PathBuf, s: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p)?;
    f.write_all(s.as_bytes())?;
    drop(f);
    restrict(p, 0o600); // mode() above only applies when the file is created
    Ok(())
}

#[cfg(unix)]
fn restrict(p: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict(_: &std::path::Path, _: u32) {}

#[cfg(not(unix))]
fn write_private(p: &PathBuf, s: &str) -> Result<()> {
    Ok(std::fs::write(p, s)?)
}

pub fn clear() -> Result<()> {
    let p = path()?;
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}
