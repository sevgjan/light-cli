//! Thin client for the (undocumented) JSON:API behind dashboard.thelightphone.com.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

pub const DEFAULT_BASE: &str = "https://production.lightphonecloud.com/api";
const MEDIA: &str = "application/vnd.api+json";

pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: Option<String>,
}

/// One JSON:API resource object: `id` plus loosely-typed attributes.
#[derive(Clone, Debug)]
pub struct Res {
    pub id: String,
    pub attrs: Value,
}

impl Res {
    fn from(v: &Value) -> Res {
        let id = match v.get("id") {
            Some(Value::String(s)) => s.clone(),
            Some(o) if !o.is_null() => o.to_string(),
            _ => String::new(),
        };
        Res { id, attrs: v.get("attributes").cloned().unwrap_or(Value::Null) }
    }

    /// First attribute among `keys` rendered as a string.
    pub fn get(&self, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|k| match self.attrs.get(*k)? {
            Value::String(s) if !s.is_empty() => Some(clean(s)),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
    }

    pub fn or_dash(&self, keys: &[&str]) -> String {
        self.get(keys).unwrap_or_else(|| "-".into())
    }

    pub fn raw(&self) -> Value {
        json!({ "id": self.id, "attributes": self.attrs })
    }
}

pub fn resources(v: &Value) -> Vec<Res> {
    match v.get("data") {
        Some(Value::Array(a)) => a.iter().map(Res::from).collect(),
        Some(o @ Value::Object(_)) => vec![Res::from(o)],
        _ => vec![],
    }
}

impl Client {
    pub fn new(token: Option<String>) -> Client {
        let base = std::env::var("LIGHT_API_URL").unwrap_or_else(|_| DEFAULT_BASE.into());
        if !secure_url(&base) {
            eprintln!("error: LIGHT_API_URL must be https (http is only allowed for localhost)");
            std::process::exit(1);
        }
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("light-cli/", env!("CARGO_PKG_VERSION")))
            .build();
        Client { agent, base: base.trim_end_matches('/').into(), token }
    }

    pub fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let mut req = self.agent.request(method, &url).set("Accept", MEDIA).set("Content-Type", MEDIA);
        if let Some(t) = &self.token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let res = match body {
            Some(b) => req.send_string(&b.to_string()),
            None => req.call(),
        };
        match res {
            Ok(r) => {
                let text = r.into_string().context("reading response")?;
                if text.trim().is_empty() {
                    Ok(Value::Null)
                } else {
                    serde_json::from_str(&text).with_context(|| format!("bad JSON from {path}"))
                }
            }
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                if code == 401 || code == 403 {
                    bail!("not authorized ({code}): run `lpt login` again{}", detail(&text));
                }
                bail!("{method} {path} failed ({code}){}", detail(&text))
            }
            Err(e) => Err(anyhow!("network error calling {path}: {e}")),
        }
    }

    fn get(&self, path: &str) -> Result<Value> {
        self.request("GET", path, None)
    }

    // ---- auth -------------------------------------------------------------

    /// Returns (token, user id).
    pub fn login(&self, email: &str, password: &str) -> Result<(String, String)> {
        let v = self.request("POST", "/authorizations", Some(json!({ "email": email, "password": password })))?;
        let token = v["included"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["type"] == "tokens"))
            .and_then(|t| t["attributes"]["token"].as_str())
            .ok_or_else(|| anyhow!("login response had no token"))?;
        let id = v["data"]["id"].as_str().unwrap_or_default();
        Ok((token.to_string(), id.to_string()))
    }

    pub fn revoke(&self) -> Result<()> {
        self.request("DELETE", "/tokens/revoke", None).map(|_| ())
    }

    pub fn current_user(&self) -> Result<Res> {
        resources(&self.get("/users/current")?).into_iter().next().ok_or_else(|| anyhow!("no user in response"))
    }

    // ---- tools ------------------------------------------------------------

    pub fn tools(&self) -> Result<Vec<Res>> {
        Ok(resources(&self.get("/third_party_tools")?))
    }

    pub fn tags(&self) -> Result<Vec<Res>> {
        Ok(resources(&self.get("/third_party_tool_tags")?))
    }

    pub fn create_tool(&self, title: &str, description: &str, repo: &str, tag_ids: &[String]) -> Result<Res> {
        let body = json!({"data": {"type": "third_party_tools", "attributes": {
            "title": title, "description": description, "repo": repo, "tag_ids": tag_ids }}});
        resources(&self.request("POST", "/third_party_tools", Some(body))?)
            .into_iter().next().ok_or_else(|| anyhow!("create returned no tool"))
    }

    pub fn update_tool(&self, id: &str, attrs: Value) -> Result<()> {
        let body = json!({"data": {"type": "third_party_tools", "id": id, "attributes": attrs}});
        self.request("PATCH", &format!("/third_party_tools/{id}"), Some(body)).map(|_| ())
    }

    pub fn versions(&self, tool: &str) -> Result<Vec<Res>> {
        Ok(resources(&self.get(&format!("/third_party_tools/{tool}/versions"))?))
    }

    pub fn builds(&self, tool: &str) -> Result<Vec<Res>> {
        Ok(resources(&self.get(&format!("/third_party_tools/{tool}/builds"))?))
    }

    pub fn build(&self, tool: &str, build: &str) -> Result<Res> {
        resources(&self.get(&format!("/third_party_tools/{tool}/builds/{build}"))?)
            .into_iter().next().ok_or_else(|| anyhow!("build not found"))
    }

    pub fn submit_build(&self, tool: &str, git_ref: &str, description: &str) -> Result<()> {
        let body = json!({"data": {"type": "third_party_tool_versions", "attributes": {
            "git_ref": git_ref, "description": description }}});
        self.request("POST", &format!("/third_party_tools/{tool}/versions"), Some(body)).map(|_| ())
    }

    pub fn retry_build(&self, tool: &str, build: &str) -> Result<()> {
        self.request("POST", &format!("/third_party_tools/{tool}/builds/{build}/retry"), None).map(|_| ())
    }

    pub fn apk_url(&self, tool: &str, build: &str) -> Result<String> {
        let v = self.get(&format!("/third_party_tools/{tool}/builds/{build}/apk"))?;
        v.get("presigned_get_url")
            .or_else(|| v["data"]["attributes"].get("presigned_get_url"))
            .and_then(|u| u.as_str())
            .map(String::from)
            .ok_or_else(|| anyhow!("this build has no APK available"))
    }

    // ---- images -----------------------------------------------------------

    pub fn images(&self, tool: &str) -> Result<Vec<Res>> {
        Ok(resources(&self.get(&format!("/third_party_tools/{tool}/images"))?))
    }

    pub fn upload_image(&self, tool: &str, filename: &str, content_type: &str, bytes: &[u8]) -> Result<()> {
        let body = json!({"data": {"type": "third_party_tool_image_uploads", "attributes": {
            "filename": filename, "content_type": content_type }}});
        let v = self.request("POST", &format!("/third_party_tools/{tool}/images"), Some(body))?;
        let attrs = &v["data"]["attributes"];
        let put = attrs["presigned_url"].as_str().ok_or_else(|| anyhow!("no presigned_url in response"))?;
        if !secure_url(put) {
            bail!("refusing to upload to a non-https URL");
        }
        let token = attrs["upload_token"].as_str().ok_or_else(|| anyhow!("no upload_token in response"))?;
        self.agent
            .put(put)
            .set("Content-Type", content_type)
            .set("x-amz-acl", "public-read")
            .send_bytes(bytes)
            .map_err(|e| match e {
                // never echo the presigned URL: its query string carries the signature
                ureq::Error::Status(code, r) => {
                    let body = r.into_string().unwrap_or_default();
                    anyhow!("image upload to storage failed ({code}) {}", body.chars().take(300).collect::<String>())
                }
                ureq::Error::Transport(t) => anyhow!("image upload to storage failed: {}", t.kind()),
            })?;
        let confirm = json!({"data": {"type": "third_party_tool_image_uploads", "attributes": {"upload_token": token}}});
        self.request("POST", &format!("/third_party_tools/{tool}/images/confirm"), Some(confirm)).map(|_| ())
    }

    pub fn delete_image(&self, tool: &str, image: &str) -> Result<()> {
        self.request("DELETE", &format!("/third_party_tools/{tool}/images/{image}"), None).map(|_| ())
    }

    /// Plain download (presigned URL, no auth header).
    pub fn download(&self, url: &str) -> Result<Vec<u8>> {
        if !secure_url(url) {
            bail!("refusing to download from a non-https URL");
        }
        let r = self.agent.get(url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => anyhow!("download failed ({code})"),
            ureq::Error::Transport(t) => anyhow!("download failed: {}", t.kind()),
        })?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut r.into_reader(), &mut buf)?;
        Ok(buf)
    }
}

/// https only; plain http is tolerated for localhost (tests against a mock).
fn secure_url(u: &str) -> bool {
    u.starts_with("https://") || u.starts_with("http://127.0.0.1") || u.starts_with("http://localhost")
}

/// Strip control characters (terminal escape sequences) from server-supplied text.
pub fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

fn detail(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return String::new() };
    let msgs: Vec<String> = v["errors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["detail"].as_str().or_else(|| e["title"].as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if msgs.is_empty() { String::new() } else { format!(": {}", clean(&msgs.join("; "))) }
}
