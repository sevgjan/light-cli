mod api;
mod auth;
mod project;

use anyhow::{anyhow, bail, Context, Result};
use api::{Client, Res};
use clap::{Parser, Subcommand};
use project::{norm_repo, Project};
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Write};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "lpt", version, about = "Submit and manage Light Phone tools from the terminal")]
struct Cli {
    /// Print raw JSON instead of tables
    #[arg(long, global = true)]
    json: bool,
    /// Skip confirmation prompts
    #[arg(short = 'y', long, global = true)]
    yes: bool,
    /// Tool to act on: id or title. Default: the tool whose repo matches this git checkout
    #[arg(short, long, global = true)]
    tool: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sign in with your Light account (LIGHT_EMAIL / LIGHT_PASSWORD for non-interactive use)
    Login,
    /// Revoke the token and forget it
    Logout,
    /// Show the signed-in account
    Whoami,
    /// List your submitted tools
    List,
    /// Tool details: versions, builds, images
    Show,
    /// List available tool tags
    Tags,
    /// Register this repo as a new tool
    Create {
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Tag name (repeatable, see `tags`)
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Change title, description or tags
    Edit {
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Manage dashboard screenshots
    Images {
        #[command(subcommand)]
        cmd: ImagesCmd,
    },
    /// Run the local preflight checks without submitting
    Check,
    /// Submit HEAD for a build
    Submit {
        /// Changelog (default: last commit subject)
        #[arg(short, long)]
        message: Option<String>,
        /// Git ref to build (default: HEAD's full SHA)
        #[arg(long)]
        git_ref: Option<String>,
        /// Wait for the build to finish
        #[arg(short, long)]
        wait: bool,
        /// Submit even if preflight reports errors
        #[arg(long)]
        force: bool,
    },
    /// List builds, or show one (with its error log)
    Builds { build: Option<String> },
    /// Watch the latest build until it finishes (reattach after Ctrl-C)
    Wait,
    /// Retry a build (default: the latest)
    Retry { build: Option<String> },
    /// Download the signed APK of a build (default: the latest successful)
    Download {
        build: Option<String>,
        #[arg(short, long)]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
enum ImagesCmd {
    List,
    /// Upload PNG/JPEG/WebP files
    Add { files: Vec<String> },
    /// Delete an image by id
    Rm { id: String },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Login => return login(),
        Cmd::Logout => return logout(),
        Cmd::Check => {
            let ok = print_report(&Project::find()?, true)?;
            return if ok { Ok(()) } else { Err(anyhow!("preflight failed")) };
        }
        _ => {}
    }
    let (token, _) = auth::load().ok_or_else(|| anyhow!("not signed in: run `lpt login`"))?;
    let c = Client::new(Some(token));
    match &cli.cmd {
        Cmd::Login | Cmd::Logout => unreachable!(),
        Cmd::Whoami => {
            let u = c.current_user()?;
            if cli.json {
                return pj(&u.raw());
            }
            println!("{}", u.or_dash(&["email"]));
            let roles = u.attrs["roles"].to_string();
            let dev = roles.contains("developer");
            println!("roles: {roles}\ndeveloper mode: {}", if dev { "on" } else { "not detected (enable it on the dashboard)" });
            Ok(())
        }
        Cmd::List => list(&c, &cli),
        Cmd::Tags => {
            let tags = c.tags()?;
            if cli.json {
                return pj(&Value::Array(tags.iter().map(Res::raw).collect()));
            }
            table(&["ID", "NAME"], tags.iter().map(|t| vec![t.id.clone(), t.or_dash(&["name", "title", "label"])]).collect());
            Ok(())
        }
        Cmd::Show => show(&c, &cli),
        Cmd::Create { title, description, tags } => create(&c, &cli, title, description, tags),
        Cmd::Edit { title, description, tags } => edit(&c, &cli, title, description, tags),
        Cmd::Images { cmd } => images(&c, &cli, cmd),
        Cmd::Check => unreachable!(),
        Cmd::Submit { message, git_ref, wait, force } => submit(&c, &cli, message, git_ref, *wait, *force),
        Cmd::Builds { build } => builds(&c, &cli, build.as_deref()),
        Cmd::Wait => {
            let tool = resolve_tool(&c, &cli)?;
            let latest = c.builds(&tool.id)?.into_iter().max_by_key(|b| b.get(&["inserted_at"])).context("no builds yet")?;
            println!("watching build {} ({})", latest.id, short(&latest.or_dash(&["git_ref"])));
            wait_for_build(&c, &tool.id, &latest.or_dash(&["git_ref"]))
        }
        Cmd::Retry { build } => {
            let tool = resolve_tool(&c, &cli)?;
            let b = match build {
                Some(b) => b.clone(),
                None => c.builds(&tool.id)?.into_iter().max_by_key(|b| b.get(&["inserted_at"])).context("no builds yet")?.id,
            };
            c.retry_build(&tool.id, &b)?;
            println!("build {b} restarted");
            Ok(())
        }
        Cmd::Download { build, output } => download(&c, &cli, build.as_deref(), output.as_deref()),
    }
}

// ---- auth -------------------------------------------------------------------

fn login() -> Result<()> {
    let email = match std::env::var("LIGHT_EMAIL") {
        Ok(e) => e,
        Err(_) => prompt("Light account email: ")?,
    };
    let password = match std::env::var("LIGHT_PASSWORD") {
        Ok(p) => p,
        Err(_) => rpassword::prompt_password("Password: ")?,
    };
    let (token, _) = Client::new(None).login(email.trim(), &password)?;
    auth::save(&token, email.trim())?;
    println!("signed in as {}", email.trim());
    Ok(())
}

fn logout() -> Result<()> {
    if let Some((t, _)) = auth::load() {
        if let Err(e) = Client::new(Some(t)).revoke() {
            eprintln!("warning: could not revoke token on the server: {e:#}");
        }
    }
    auth::clear()?;
    println!("signed out");
    Ok(())
}

// ---- tools --------------------------------------------------------------------

fn resolve_tool(c: &Client, cli: &Cli) -> Result<Res> {
    let tools = c.tools()?;
    if let Some(sel) = &cli.tool {
        let l = sel.to_lowercase();
        let hits: Vec<&Res> = tools
            .iter()
            .filter(|t| t.id == *sel || t.get(&["title"]).is_some_and(|x| x.to_lowercase() == l)
                || t.get(&["namespace", "package", "package_name"]).is_some_and(|x| x == *sel))
            .collect();
        return match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(anyhow!("no tool matches `{sel}` (see `lpt list`)")),
            _ => Err(anyhow!("`{sel}` matches several tools; use the id")),
        };
    }
    let p = Project::find().context("run this inside a tool repo, or pass --tool")?;
    let origin = norm_repo(&p.origin()?);
    tools
        .into_iter()
        .find(|t| t.get(&["repo", "repo_url", "repository"]).is_some_and(|r| norm_repo(&r) == origin))
        .ok_or_else(|| anyhow!("no registered tool for {origin}: run `lpt create`"))
}

fn list(c: &Client, cli: &Cli) -> Result<()> {
    let tools = c.tools()?;
    if cli.json {
        return pj(&Value::Array(tools.iter().map(Res::raw).collect()));
    }
    if tools.is_empty() {
        println!("no tools yet: `lpt create` in a tool repo");
        return Ok(());
    }
    let mut rows = vec![];
    for t in &tools {
        let versions = c.versions(&t.id).unwrap_or_default();
        let latest = versions.iter().max_by_key(|v| ver_code(v)).map(|v| v.or_dash(&["version_name", "name", "version"])).unwrap_or("-".into());
        let state = versions.iter().max_by_key(|v| ver_code(v)).map(|v| v.or_dash(&["status"])).unwrap_or("-".into());
        rows.push(vec![t.id.clone(), t.or_dash(&["title"]), t.or_dash(&["status"]), latest, state, t.or_dash(&["repo"])]);
    }
    table(&["ID", "TITLE", "STATUS", "LATEST", "VERSION STATUS", "REPO"], rows);
    Ok(())
}

fn show(c: &Client, cli: &Cli) -> Result<()> {
    let t = resolve_tool(c, cli)?;
    let versions = c.versions(&t.id)?;
    let builds = c.builds(&t.id)?;
    let images = c.images(&t.id).unwrap_or_default();
    if cli.json {
        return pj(&json!({
            "tool": t.raw(),
            "versions": versions.iter().map(Res::raw).collect::<Vec<_>>(),
            "builds": builds.iter().map(Res::raw).collect::<Vec<_>>(),
            "images": images.iter().map(Res::raw).collect::<Vec<_>>(),
        }));
    }
    println!("{}  (id {})", t.or_dash(&["title"]), t.id);
    println!("status:  {}", t.or_dash(&["status"]));
    println!("repo:    {}", t.or_dash(&["repo"]));
    println!("package: {}", t.or_dash(&["namespace", "package", "package_name"]));
    println!("tags:    {}", t.attrs["tags"].as_array().map(|a| a.iter().map(|x| x.as_str().map(String::from).unwrap_or(x.to_string())).collect::<Vec<_>>().join(", ")).unwrap_or_default());
    println!("images:  {}{}", images.len(), if images.is_empty() { "  (at least one is required for approval)" } else { "" });
    println!("\nVersions");
    table(&["ID", "VERSION", "CODE", "STATUS", "CHANGELOG"], versions.iter().map(|v| vec![
        v.id.clone(), v.or_dash(&["version_name", "name", "version"]), v.or_dash(&["version_code"]), v.or_dash(&["status"]), v.or_dash(&["description"]),
    ]).collect());
    println!("\nBuilds");
    print_builds(&builds);
    Ok(())
}

fn print_builds(builds: &[Res]) {
    let mut b: Vec<&Res> = builds.iter().collect();
    b.sort_by_key(|x| std::cmp::Reverse(x.get(&["inserted_at"])));
    table(&["ID", "GIT REF", "STATUS", "SUBMITTED"], b.iter().map(|x| vec![
        x.id.clone(), short(&x.or_dash(&["git_ref"])), x.or_dash(&["status"]), x.or_dash(&["inserted_at"]),
    ]).collect());
}

fn builds(c: &Client, cli: &Cli, which: Option<&str>) -> Result<()> {
    let t = resolve_tool(c, cli)?;
    match which {
        None => {
            let b = c.builds(&t.id)?;
            if cli.json {
                return pj(&Value::Array(b.iter().map(Res::raw).collect()));
            }
            print_builds(&b);
        }
        Some(id) => {
            let b = c.build(&t.id, id)?;
            if cli.json {
                return pj(&b.raw());
            }
            println!("build {}  {}  {}", b.id, short(&b.or_dash(&["git_ref"])), b.or_dash(&["status"]));
            print_error(&b);
        }
    }
    Ok(())
}

fn print_error(b: &Res) {
    match b.attrs.get("error") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => println!("\n{}", api::clean(s)),
        Some(v) => println!("\n{}", api::clean(&serde_json::to_string_pretty(v).unwrap_or_default())),
    }
}

fn ver_code(v: &Res) -> i64 {
    v.get(&["version_code"]).and_then(|s| s.parse().ok()).unwrap_or(-1)
}

fn tag_ids(c: &Client, names: &[String]) -> Result<Vec<String>> {
    if names.is_empty() {
        return Ok(vec![]);
    }
    let all = c.tags()?;
    names
        .iter()
        .map(|n| {
            all.iter()
                .find(|t| t.id == *n || t.get(&["name", "title", "label"]).is_some_and(|x| x.eq_ignore_ascii_case(n)))
                .map(|t| t.id.clone())
                .ok_or_else(|| {
                    let names: Vec<String> = all.iter().filter_map(|t| t.get(&["name", "title", "label"])).collect();
                    anyhow!("unknown tag `{n}`. Available: {}", names.join(", "))
                })
        })
        .collect()
}

/// Numbered multi-select over the dashboard's tags.
fn pick_tags(c: &Client) -> Result<Vec<String>> {
    let all = c.tags()?;
    if all.is_empty() {
        return Ok(vec![]);
    }
    println!("Tags:");
    for (i, t) in all.iter().enumerate() {
        println!("  {:>2}) {}", i + 1, t.or_dash(&["name", "title", "label"]));
    }
    loop {
        let a = prompt("Pick tags by number, comma-separated (Enter for none): ")?;
        if a.is_empty() {
            return Ok(vec![]);
        }
        let picked: Option<Vec<String>> = a
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|x| !x.is_empty())
            .map(|x| x.parse::<usize>().ok().and_then(|n| all.get(n.checked_sub(1)?)).map(|t| t.id.clone()))
            .collect();
        match picked {
            Some(mut ids) => {
                ids.dedup();
                return Ok(ids);
            }
            None => println!("  enter numbers between 1 and {}", all.len()),
        }
    }
}

fn create(c: &Client, cli: &Cli, title: &Option<String>, description: &Option<String>, tags: &[String]) -> Result<()> {
    let p = Project::find()?;
    let m = p.manifest()?;
    let repo = project::https_url(&p.origin().context("no `origin` remote: push the repo to GitHub first")?);
    let ask = !cli.yes && std::io::stdin().is_terminal();
    let title = match title {
        Some(t) => t.clone(),
        None if ask => prompt_default("Tool name", &m.label)?,
        None => m.label.clone(),
    };
    let blurb = p.readme_blurb().unwrap_or_default();
    let description = match description {
        Some(d) => d.clone(),
        None if ask => prompt_default("Description", &blurb)?,
        None => blurb,
    };
    if title.trim().is_empty() {
        bail!("a tool name is required (--title)");
    }
    if c.tools()?.iter().any(|t| t.get(&["repo"]).is_some_and(|r| norm_repo(&r) == norm_repo(&repo))) {
        bail!("{repo} is already registered");
    }
    let ids = if tags.is_empty() && !cli.yes && std::io::stdin().is_terminal() { pick_tags(c)? } else { tag_ids(c, tags)? };
    println!("Create tool on the Light dashboard:\n  title:       {title}\n  package:     {} (read from the default branch, can't be changed later)\n  repo:        {repo}\n  description: {description}", m.id);
    if !cli.yes && !confirm("Create it?")? {
        bail!("aborted");
    }
    let t = c.create_tool(&title, &description, &repo, &ids)?;
    println!("created tool {}. Next: `lpt images add shot.png`, then `lpt submit`", t.id);
    Ok(())
}

fn edit(c: &Client, cli: &Cli, title: &Option<String>, description: &Option<String>, tags: &[String]) -> Result<()> {
    let t = resolve_tool(c, cli)?;
    let mut a = serde_json::Map::new();
    if let Some(x) = title { a.insert("title".into(), json!(x)); }
    if let Some(x) = description { a.insert("description".into(), json!(x)); }
    if !tags.is_empty() { a.insert("tag_ids".into(), json!(tag_ids(c, tags)?)); }
    if a.is_empty() {
        bail!("nothing to change: pass --title, --description or --tag");
    }
    c.update_tool(&t.id, Value::Object(a))?;
    println!("updated");
    Ok(())
}

fn images(c: &Client, cli: &Cli, cmd: &ImagesCmd) -> Result<()> {
    let t = resolve_tool(c, cli)?;
    match cmd {
        ImagesCmd::List => {
            let imgs = c.images(&t.id)?;
            if cli.json {
                return pj(&Value::Array(imgs.iter().map(Res::raw).collect()));
            }
            table(&["ID", "POS", "TYPE", "URL"], imgs.iter().map(|i| vec![i.id.clone(), i.or_dash(&["position"]), i.or_dash(&["content_type"]), i.or_dash(&["url"])]).collect());
        }
        ImagesCmd::Add { files } => {
            if files.is_empty() { bail!("pass at least one image file"); }
            for f in files {
                let ext = f.rsplit('.').next().unwrap_or("").to_lowercase();
                let ct = match ext.as_str() { "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "webp" => "image/webp", _ => bail!("{f}: only png, jpg and webp are supported") };
                let bytes = std::fs::read(f).with_context(|| format!("reading {f}"))?;
                let name = std::path::Path::new(f).file_name().and_then(|n| n.to_str()).unwrap_or(f);
                c.upload_image(&t.id, name, ct, &bytes)?;
                println!("uploaded {name}");
            }
        }
        ImagesCmd::Rm { id } => {
            c.delete_image(&t.id, id)?;
            println!("deleted");
        }
    }
    Ok(())
}

// ---- submit -----------------------------------------------------------------

fn latest_sdk() -> Option<String> {
    let r = ureq::get("https://raw.githubusercontent.com/lightphone/light-sdk/main/gradle.properties").timeout(Duration::from_secs(10)).call().ok()?;
    r.into_string().ok()?.lines().find_map(|l| l.strip_prefix("sdkVersion=").map(|v| v.trim().to_string()))
}

fn print_report(p: &Project, verbose: bool) -> Result<bool> {
    let r = p.check(latest_sdk().as_deref());
    for w in &r.warnings { println!("  warn  {w}"); }
    for e in &r.errors { println!("  FAIL  {e}"); }
    if verbose && r.errors.is_empty() {
        let m = p.manifest()?;
        println!("  ok    {} {} (versionCode {}), HEAD {}", m.id, m.version_name, m.version_code, short(&p.head()?));
    }
    Ok(r.errors.is_empty())
}

fn submit(c: &Client, cli: &Cli, message: &Option<String>, git_ref: &Option<String>, wait: bool, force: bool) -> Result<()> {
    let p = Project::find()?;
    println!("Preflight");
    if !print_report(&p, true)? && !force {
        bail!("fix the failures above (or --force)");
    }
    let m = p.manifest()?;
    let sha = p.head()?;
    let git_ref = git_ref.clone().unwrap_or_else(|| sha.clone());
    if git_ref == sha {
        match p.pushed(&sha) {
            Ok(true) => {}
            Ok(false) if force => println!("  warn  HEAD {} is not the tip of a branch on origin", short(&sha)),
            Ok(false) => bail!("HEAD {} is not the tip of a branch on origin: push first (Light builds from GitHub)", short(&sha)),
            Err(e) if force => println!("  warn  could not verify the push: {e:#}"),
            Err(e) => return Err(e.context("could not verify HEAD is pushed (--force skips this)")),
        }
    }
    let tool = resolve_tool(c, cli)?;
    let versions = c.versions(&tool.id)?;
    if let Some(prev) = versions.iter().map(ver_code).max().filter(|v| *v >= 0) {
        if m.version_code <= prev && !force {
            bail!("versionCode {} must be higher than the last version ({prev}): bump it in tool/lighttool.toml", m.version_code);
        }
    }
    let existing = c.builds(&tool.id)?;
    if let Some(b) = existing.iter().find(|b| !is_done(&b.or_dash(&["status"]))) {
        bail!("build {} is still {}: the server only allows one active build", b.id, b.or_dash(&["status"]));
    }
    if c.images(&tool.id).map(|i| i.is_empty()).unwrap_or(false) {
        println!("  warn  the tool has no images yet (required for approval): `lpt images add`");
    }
    let changelog = match message {
        Some(m) => m.clone(),
        None => p.last_message()?,
    };
    println!("\nSubmit {} {} to \"{}\"\n  git ref:   {}\n  changelog: {}", m.id, m.version_name, tool.or_dash(&["title"]), git_ref, changelog);
    if !cli.yes && !confirm("Submit this build?")? {
        bail!("aborted");
    }
    c.submit_build(&tool.id, &git_ref, &changelog)?;
    println!("build submitted");
    if wait {
        wait_for_build(c, &tool.id, &git_ref)?;
    } else {
        println!("track it with `lpt builds`");
    }
    Ok(())
}

fn is_done(status: &str) -> bool {
    is_failed(status) || is_ok(status)
}
fn is_failed(s: &str) -> bool {
    let s = s.to_lowercase();
    s.contains("fail") || s.contains("error") || s.contains("reject") || s.contains("cancel")
}
fn is_ok(s: &str) -> bool {
    let s = s.to_lowercase();
    ["success", "succeeded", "complete", "built", "done", "ready", "pending_approval", "approved"].iter().any(|k| s.contains(k))
}

fn wait_for_build(c: &Client, tool: &str, git_ref: &str) -> Result<()> {
    let tty = std::io::stdout().is_terminal();
    let start = Instant::now();
    let mut last = String::new();
    let mut status = String::from("submitted");
    let mut last_poll: Option<Instant> = None;
    let mut tick = 0usize;
    loop {
        if last_poll.map_or(true, |t| t.elapsed() >= Duration::from_secs(10)) {
            last_poll = Some(Instant::now());
            let b = c.builds(tool)?.into_iter().filter(|b| b.get(&["git_ref"]).is_some_and(|r| r == git_ref)).max_by_key(|b| b.get(&["inserted_at"]));
            if let Some(b) = b {
                status = b.or_dash(&["status"]);
                if status != last {
                    if tty { print!("\r\x1b[2K"); }
                    println!("  {} {status}", fmt_elapsed(start.elapsed()));
                    last = status.clone();
                }
                if is_failed(&status) {
                    print_error(&b);
                    bail!("build failed (`lpt retry` to try again)");
                }
                if is_ok(&status) {
                    println!("build finished: it now waits for Light's review. `lpt download` fetches the signed APK");
                    return Ok(());
                }
            }
        }
        if start.elapsed() > Duration::from_secs(45 * 60) {
            if tty { println!(); }
            bail!("gave up waiting after 45 minutes; check `lpt builds`");
        }
        if tty {
            tick += 1;
            print!("\r\x1b[2K  {} {status}{}", fmt_elapsed(start.elapsed()), ".".repeat(tick % 4));
            std::io::stdout().flush()?;
            std::thread::sleep(Duration::from_millis(500));
        } else {
            std::thread::sleep(Duration::from_secs(5));
        }
    }
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("[{}:{:02}]", s / 60, s % 60)
}

fn download(c: &Client, cli: &Cli, build: Option<&str>, output: Option<&str>) -> Result<()> {
    let t = resolve_tool(c, cli)?;
    let id = match build {
        Some(b) => b.to_string(),
        None => c.builds(&t.id)?.into_iter().filter(|b| is_ok(&b.or_dash(&["status"]))).max_by_key(|b| b.get(&["inserted_at"])).context("no successful build yet")?.id,
    };
    let bytes = c.download(&c.apk_url(&t.id, &id)?)?;
    let safe = |s: &str| -> String { s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect() };
    let out = output.map(String::from).unwrap_or_else(|| format!("{}-{}.apk", safe(&t.or_dash(&["title"])), safe(&id)));
    std::fs::write(&out, &bytes)?;
    println!("saved {out} ({} KB)", bytes.len() / 1024);
    Ok(())
}

// ---- output helpers ---------------------------------------------------------

fn short(s: &str) -> String {
    if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) { s[..8].to_string() } else { s.to_string() }
}

fn pj(v: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        println!("  (none)");
        return;
    }
    let cut = |s: &str| if s.chars().count() > 60 { format!("{}…", s.chars().take(59).collect::<String>()) } else { s.to_string() };
    let rows: Vec<Vec<String>> = rows.iter().map(|r| r.iter().map(|c| cut(c)).collect()).collect();
    let w: Vec<usize> = (0..headers.len()).map(|i| rows.iter().map(|r| r[i].chars().count()).chain([headers[i].len()]).max().unwrap()).collect();
    let line = |cells: Vec<&str>| {
        let s: Vec<String> = cells.iter().enumerate().map(|(i, c)| format!("{:<w$}", c, w = w[i])).collect();
        println!("{}", s.join("  ").trim_end());
    };
    line(headers.to_vec());
    for r in &rows { line(r.iter().map(String::as_str).collect()); }
}

fn prompt(q: &str) -> Result<String> {
    print!("{q}");
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

/// Prompt showing a (truncated) default that Enter accepts.
fn prompt_default(label: &str, default: &str) -> Result<String> {
    let shown: String = if default.chars().count() > 50 { format!("{}…", default.chars().take(49).collect::<String>()) } else { default.to_string() };
    let a = prompt(&format!("{label} [{shown}]: "))?;
    Ok(if a.is_empty() { default.to_string() } else { a })
}

fn confirm(q: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal: pass --yes to confirm");
    }
    Ok(matches!(prompt(&format!("{q} [y/N] "))?.to_lowercase().as_str(), "y" | "yes"))
}
