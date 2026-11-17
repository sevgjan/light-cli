//! Local tool-repo inspection and the preflight checks.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Manifest {
    pub id: String,
    pub label: String,
    pub version_code: i64,
    pub version_name: String,
    pub server_package: Option<String>,
}

pub struct Project {
    pub root: PathBuf,
}

pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Project {
    /// Nearest ancestor of cwd that holds tool/lighttool.toml.
    pub fn find() -> Result<Project> {
        let cwd = std::env::current_dir()?;
        let root = cwd
            .ancestors()
            .find(|d| d.join("tool/lighttool.toml").is_file())
            .context("no tool/lighttool.toml found here or in a parent directory")?;
        Ok(Project { root: root.to_path_buf() })
    }

    pub fn manifest(&self) -> Result<Manifest> {
        let text = std::fs::read_to_string(self.root.join("tool/lighttool.toml"))?;
        let v: toml::Value = text.parse().context("tool/lighttool.toml is not valid TOML")?;
        let t = v.get("tool").context("lighttool.toml has no [tool] table")?;
        let s = |k: &str| t.get(k).and_then(|x| x.as_str()).map(String::from);
        Ok(Manifest {
            id: s("id").context("lighttool.toml: missing id")?,
            label: s("label").unwrap_or_default(),
            version_code: t.get("versionCode").and_then(|x| x.as_integer()).context("lighttool.toml: missing versionCode")?,
            version_name: s("versionName").unwrap_or_default(),
            server_package: s("serverPackage"),
        })
    }

    pub fn sdk_version(&self) -> Option<String> {
        prop(&self.root.join("gradle.properties"), "sdkVersion")
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git").arg("-C").arg(&self.root).args(args).output().context("running git")?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn head(&self) -> Result<String> {
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn origin(&self) -> Result<String> {
        self.git(&["remote", "get-url", "origin"])
    }

    pub fn last_message(&self) -> Result<String> {
        self.git(&["log", "-1", "--format=%s"])
    }

    pub fn dirty(&self) -> Result<bool> {
        Ok(!self.git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty())
    }

    /// Is `sha` the tip of some branch on origin? (network)
    pub fn pushed(&self, sha: &str) -> Result<bool> {
        Ok(self.git(&["ls-remote", "--heads", "origin"])?.lines().any(|l| l.starts_with(sha)))
    }

    /// First prose paragraph of README.md, for a default description.
    pub fn readme_blurb(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join("README.md")).ok()?;
        text.split("\n\n")
            .map(str::trim)
            .find(|p| !p.is_empty() && !p.starts_with('#') && !p.starts_with('-') && !p.starts_with('!') && !p.starts_with('['))
            .map(|p| p.replace('\n', " "))
    }

    pub fn check(&self, latest_sdk: Option<&str>) -> Report {
        let mut r = Report { errors: vec![], warnings: vec![] };
        match self.manifest() {
            Err(e) => r.errors.push(format!("{e:#}")),
            Ok(m) => {
                if m.id.split('.').count() < 2 {
                    r.errors.push(format!("tool id `{}` needs at least two dotted segments", m.id));
                }
                if m.id.starts_with("com.lightphone") || m.id.starts_with("org.thelightphone") || m.id.starts_with("com.thelightphone") {
                    r.errors.push(format!("tool id `{}` is in a namespace reserved for Light", m.id));
                }
                if !is_semver(&m.version_name) {
                    r.errors.push(format!("versionName `{}` must be plain x.y.z", m.version_name));
                }
                if m.server_package.as_deref().is_some_and(|s| s != "com.lightos") {
                    r.errors.push("serverPackage must be com.lightos before submitting".into());
                }
            }
        }
        match self.sdk_version() {
            None => r.errors.push("gradle.properties is missing sdkVersion (the build preflight needs it)".into()),
            Some(v) => {
                if let Some(l) = latest_sdk {
                    if v != l {
                        r.warnings.push(format!("sdkVersion is {v}, latest light-sdk is {l}"));
                    }
                }
            }
        }
        if !["LICENSE", "LICENSE.md", "LICENSE.txt"].iter().any(|f| self.root.join(f).is_file()) {
            r.errors.push("no LICENSE file (tools must be open source)".into());
        }
        match std::fs::read_to_string(self.root.join("README.md")) {
            Err(_) => r.warnings.push("no README.md".into()),
            Ok(t) => {
                let heads: Vec<String> = t.lines().filter(|l| l.starts_with('#')).map(|l| l.to_lowercase()).collect();
                for h in ["privacy", "licen", "build"] {
                    if !heads.iter().any(|l| l.contains(h)) {
                        r.warnings.push(format!("README has no `## {}` section", capitalise(if h == "licen" { "license" } else { h })));
                    }
                }
            }
        }
        match self.dirty() {
            Ok(true) => r.errors.push("uncommitted changes: the build uses a pushed commit, not your working tree".into()),
            Ok(false) => {}
            Err(e) => r.errors.push(format!("not a usable git repo: {e:#}")),
        }
        r
    }
}

pub fn prop(path: &Path, key: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()?.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

fn is_semver(s: &str) -> bool {
    let p: Vec<&str> = s.split('.').collect();
    p.len() == 3 && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Normalise a git remote to `github.com/owner/repo` for comparison.
pub fn norm_repo(url: &str) -> String {
    let u = url.trim().trim_end_matches('/').trim_end_matches(".git").to_lowercase();
    let u = u
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("ssh://")
        .trim_start_matches("git@");
    u.replacen(':', "/", 1).trim_start_matches("www.").to_string()
}

pub fn https_url(origin: &str) -> String {
    format!("https://{}", norm_repo(origin))
}
