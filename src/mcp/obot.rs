//! Obot HTTP client: config load, health probe, list installed MCP servers,
//! catalog search, and create-from-catalog-entry.

use super::types::{
    CatalogEntryView, CreateServerResult, McpServerView, ObotConfig, ObotHealth, ObotHealthStatus,
    ServerSource,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const SERVERS_PATH: &str = "/api/all-mcps/servers";
const SERVERS_FALLBACK_PATH: &str = "/api/mcp-servers";
const ENTRIES_PATH: &str = "/api/all-mcps/entries";
const CREATE_SERVER_PATH: &str = "/api/mcp-servers";
const REGISTRY_SERVERS_PATH: &str = "/v0.1/servers";

/// Load `~/.config/sessions/obot.toml`. Missing file → defaults (enabled, localhost:8080).
pub fn load_config(home: &Path) -> Result<ObotConfig> {
    let path = crate::paths::obot_config_path(home);
    if !path.is_file() {
        return Ok(ObotConfig::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read obot config {}", path.display()))?;
    let mut cfg: ObotConfig =
        toml::from_str(&text).with_context(|| format!("parse obot config {}", path.display()))?;
    if cfg.base_url.trim().is_empty() {
        cfg.base_url = ObotConfig::default().base_url;
    }
    if cfg.open_admin_path.trim().is_empty() {
        cfg.open_admin_path = ObotConfig::default().open_admin_path;
    }
    Ok(cfg)
}

/// Probe Obot. HTTP responses including 401/404 count as **up** (service listening).
pub fn health(home: &Path) -> Result<ObotHealth> {
    let cfg = load_config(home)?;
    health_with_config(&cfg)
}

pub fn health_with_config(cfg: &ObotConfig) -> Result<ObotHealth> {
    let token_configured = cfg.resolved_token().is_some();
    if !cfg.enabled {
        return Ok(ObotHealth {
            status: ObotHealthStatus::Disabled,
            base_url: cfg.base_url.clone(),
            detail: "Obot integration disabled in obot.toml".into(),
            token_configured,
        });
    }

    let client = http_client()?;
    let base = cfg.base_url.trim_end_matches('/');
    // Prefer a cheap API path; fall back to base URL.
    let candidates = [format!("{base}/api/"), format!("{base}/"), base.to_string()];

    let mut last_err = String::new();
    for url in &candidates {
        match client.get(url).send() {
            Ok(resp) => {
                let code = resp.status().as_u16();
                // Any HTTP response means the process is up (auth may block body).
                return Ok(ObotHealth {
                    status: ObotHealthStatus::Up,
                    base_url: cfg.base_url.clone(),
                    detail: format!("HTTP {code} from {url}"),
                    token_configured,
                });
            }
            Err(err) => {
                last_err = err.to_string();
            }
        }
    }

    Ok(ObotHealth {
        status: ObotHealthStatus::Down,
        base_url: cfg.base_url.clone(),
        detail: format!("unreachable: {last_err}"),
        token_configured,
    })
}

/// List installed servers from Obot. Returns empty on disabled / soft failures when `soft=true`.
pub fn list_servers(home: &Path) -> Result<Vec<McpServerView>> {
    let cfg = load_config(home)?;
    list_servers_with_config(&cfg)
}

pub fn list_servers_with_config(cfg: &ObotConfig) -> Result<Vec<McpServerView>> {
    if !cfg.enabled {
        return Ok(Vec::new());
    }
    let client = http_client()?;
    let base = cfg.base_url.trim_end_matches('/');
    let paths = [SERVERS_PATH, SERVERS_FALLBACK_PATH];
    let mut last_err = None;

    for path in paths {
        let url = format!("{base}{path}");
        match get_json(cfg, &client, &url) {
            Ok(Ok(value)) => return Ok(parse_servers_value(&value)),
            Ok(Err(msg)) => last_err = Some(msg),
            Err(err) => last_err = Some(err.to_string()),
        }
    }

    anyhow::bail!(
        "failed to list Obot MCP servers: {}",
        last_err.unwrap_or_else(|| "unknown error".into())
    )
}

/// List catalog entries available to deploy (`GET /api/all-mcps/entries`).
pub fn list_catalog_entries(home: &Path) -> Result<Vec<CatalogEntryView>> {
    let cfg = load_config(home)?;
    list_catalog_entries_with_config(&cfg)
}

pub fn list_catalog_entries_with_config(cfg: &ObotConfig) -> Result<Vec<CatalogEntryView>> {
    if !cfg.enabled {
        return Ok(Vec::new());
    }
    let client = http_client()?;
    let base = cfg.base_url.trim_end_matches('/');
    let url = format!("{base}{ENTRIES_PATH}");
    match get_json(cfg, &client, &url)? {
        Ok(value) => Ok(parse_catalog_entries_value(&value)),
        Err(msg) => anyhow::bail!("failed to list catalog entries: {msg}"),
    }
}

/// Search catalog entries by name / description (client-side filter).
///
/// When `query` is empty, returns all entries (capped by Obot).
pub fn search_catalog(home: &Path, query: &str) -> Result<Vec<CatalogEntryView>> {
    let mut entries = list_catalog_entries(home)?;
    filter_catalog_entries(&mut entries, query);
    Ok(entries)
}

/// Filter catalog entries in place by case-insensitive substring match.
pub fn filter_catalog_entries(entries: &mut Vec<CatalogEntryView>, query: &str) {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return;
    }
    entries.retain(|e| {
        e.name.to_ascii_lowercase().contains(&q)
            || e.short_description.to_ascii_lowercase().contains(&q)
            || e.description.to_ascii_lowercase().contains(&q)
            || e.id.to_ascii_lowercase().contains(&q)
            || e.catalog_name.to_ascii_lowercase().contains(&q)
    });
}

/// Registry search via MCP Registry API (`GET /v0.1/servers?search=`).
///
/// Useful for discovery; deploy still uses catalog entry ids from
/// [`list_catalog_entries`]. Soft-fails to empty on 404.
pub fn search_registry(home: &Path, query: &str, limit: usize) -> Result<Vec<CatalogEntryView>> {
    let cfg = load_config(home)?;
    search_registry_with_config(&cfg, query, limit)
}

pub fn search_registry_with_config(
    cfg: &ObotConfig,
    query: &str,
    limit: usize,
) -> Result<Vec<CatalogEntryView>> {
    if !cfg.enabled {
        return Ok(Vec::new());
    }
    let client = http_client()?;
    let base = cfg.base_url.trim_end_matches('/');
    // Registry path is on app base (not always under /api).
    let app_base = base.trim_end_matches("/api");
    let mut url = format!("{app_base}{REGISTRY_SERVERS_PATH}");
    let mut params = Vec::new();
    let q = query.trim();
    if !q.is_empty() {
        params.push(format!("search={}", urlencoding_minimal(q)));
    }
    if limit > 0 {
        params.push(format!("limit={limit}"));
    }
    if !params.is_empty() {
        url.push('?');
        url.push_str(&params.join("&"));
    }
    match get_json(cfg, &client, &url) {
        Ok(Ok(value)) => Ok(parse_registry_servers_value(&value)),
        Ok(Err(msg)) if msg.starts_with("404 ") => Ok(Vec::new()),
        Ok(Err(msg)) => anyhow::bail!("registry search failed: {msg}"),
        Err(err) => Err(err).context("registry search request"),
    }
}

/// Deploy a catalog entry as a personal MCP server (`POST /api/mcp-servers`).
pub fn create_server_from_entry(
    home: &Path,
    catalog_entry_id: &str,
    alias: Option<&str>,
) -> Result<CreateServerResult> {
    let cfg = load_config(home)?;
    create_server_from_entry_with_config(&cfg, catalog_entry_id, alias)
}

pub fn create_server_from_entry_with_config(
    cfg: &ObotConfig,
    catalog_entry_id: &str,
    alias: Option<&str>,
) -> Result<CreateServerResult> {
    if !cfg.enabled {
        anyhow::bail!("Obot integration disabled in obot.toml");
    }
    let id = catalog_entry_id.trim();
    if id.is_empty() {
        anyhow::bail!("catalog entry id is required");
    }
    let client = http_client()?;
    let base = cfg.base_url.trim_end_matches('/');
    let url = format!("{base}{CREATE_SERVER_PATH}");
    let mut body = serde_json::json!({
        "catalogEntryID": id,
    });
    if let Some(a) = alias.map(str::trim).filter(|s| !s.is_empty()) {
        body["alias"] = Value::String(a.to_string());
    }
    let mut req = client.post(&url).json(&body);
    if let Some(token) = cfg.resolved_token() {
        req = req.bearer_auth(token);
    }
    let resp = req
        .send()
        .with_context(|| format!("POST create MCP server {url}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        let detail = extract_error_detail(&text).unwrap_or_else(|| text.trim().to_string());
        anyhow::bail!(
            "create server failed (HTTP {}): {}",
            status.as_u16(),
            if detail.is_empty() {
                status.to_string()
            } else {
                detail
            }
        );
    }
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("decode create-server response from {url}"))?;
    parse_create_server_result(&value, alias).context("parse create-server response")
}

/// GET JSON with optional bearer token. `Ok(Ok(value))` on success parse,
/// `Ok(Err(msg))` on non-success HTTP, `Err` on transport failure.
fn get_json(
    cfg: &ObotConfig,
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<std::result::Result<Value, String>> {
    let mut req = client.get(url);
    if let Some(token) = cfg.resolved_token() {
        req = req.bearer_auth(token);
    }
    let resp = req.send().with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Ok(Err(format!("404 {url}")));
    }
    if !status.is_success() && status.as_u16() != 401 {
        let body = resp.text().unwrap_or_default();
        if body.trim().is_empty() {
            return Ok(Err(format!("HTTP {status} from {url}")));
        }
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            return Ok(Ok(value));
        }
        return Ok(Err(format!("HTTP {status} from {url}")));
    }
    // 2xx or 401 (try body anyway)
    let body = resp.text().unwrap_or_default();
    if body.trim().is_empty() {
        if status.as_u16() == 401 {
            return Ok(Err(format!("HTTP 401 from {url} (auth required)")));
        }
        return Ok(Err(format!("empty body from {url}")));
    }
    match serde_json::from_str::<Value>(&body) {
        Ok(value) => Ok(Ok(value)),
        Err(err) => Ok(Err(format!("invalid JSON from {url}: {err}"))),
    }
}

fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn extract_error_detail(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    if let Some(s) = value.get("message").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = value.get("error").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = value.get("detail").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    None
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .connect_timeout(Duration::from_secs(2))
        .user_agent(format!("sessions-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build HTTP client")
}

/// Defensive parse of Obot list payloads.
///
/// Supports:
/// - `{ "items": [ ... ] }` (canonical MCPServerList)
/// - bare `[ ... ]`
/// - `{ "servers": [ ... ] }` / `{ "data": [ ... ] }`
pub fn parse_servers_value(value: &Value) -> Vec<McpServerView> {
    let items = extract_items(value);
    let mut out = Vec::new();
    for item in items {
        if let Some(view) = parse_one_server(item) {
            out.push(view);
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn extract_items(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(arr) => arr.iter().collect(),
        Value::Object(map) => {
            for key in ["items", "servers", "data", "mcpServers", "mcp_servers"] {
                if let Some(Value::Array(arr)) = map.get(key) {
                    return arr.iter().collect();
                }
            }
            // Single server object
            if map.contains_key("id")
                || map.contains_key("connectURL")
                || map.contains_key("connect_url")
                || map.contains_key("manifest")
            {
                return vec![value];
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn parse_one_server(item: &Value) -> Option<McpServerView> {
    let obj = item.as_object()?;

    let obot_id = first_string(obj, &["id", "name", "mcpServerID", "mcp_server_id"])
        .unwrap_or_else(|| "unknown".into());

    let manifest = obj.get("manifest").and_then(|v| v.as_object());
    let display_name = manifest
        .and_then(|m| first_string(m, &["name", "Name"]))
        .or_else(|| first_string(obj, &["alias", "name", "displayName", "display_name"]))
        .unwrap_or_else(|| obot_id.clone());

    // Nested remoteConfig.url is upstream, not the gateway — only top-level connect fields.
    let connect_url = first_string(
        obj,
        &["connectURL", "connect_url", "connectUrl", "url", "URL"],
    )
    .unwrap_or_default();

    // Prefer human alias / name as agent config key; fall back to id.
    let key = first_string(obj, &["alias"])
        .filter(|s| is_safe_key(s))
        .or_else(|| {
            let slug = slugify(&display_name);
            if is_safe_key(&slug) {
                Some(slug)
            } else {
                None
            }
        })
        .or_else(|| {
            let slug = slugify(&obot_id);
            if is_safe_key(&slug) {
                Some(slug)
            } else {
                None
            }
        })
        .unwrap_or_else(|| slugify(&obot_id));

    let oauth_ok = parse_oauth_ok(obj);
    let running = parse_running(obj);

    Some(McpServerView {
        key,
        display_name,
        source: ServerSource::ObotGateway {
            obot_id,
            connect_url,
        },
        oauth_ok,
        running,
    })
}

fn parse_oauth_ok(obj: &serde_json::Map<String, Value>) -> Option<bool> {
    if let Some(v) = obj.get("missingOAuthCredentials").and_then(|v| v.as_bool()) {
        return Some(!v);
    }
    if let Some(v) = obj
        .get("oauthCredentialConfigured")
        .and_then(|v| v.as_bool())
    {
        return Some(v);
    }
    if let Some(v) = obj.get("configured").and_then(|v| v.as_bool()) {
        // Weak signal only.
        return Some(v);
    }
    None
}

fn parse_running(obj: &serde_json::Map<String, Value>) -> Option<bool> {
    if let Some(status) = obj.get("deploymentStatus").and_then(|v| v.as_str()) {
        let s = status.to_ascii_lowercase();
        if s.contains("ready") || s == "running" {
            return Some(true);
        }
        if s.contains("fail") || s.contains("error") || s == "pending" || s.contains("progress") {
            return Some(false);
        }
    }
    if let Some(n) = obj
        .get("deploymentReadyReplicas")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            obj.get("deploymentAvailableReplicas")
                .and_then(|v| v.as_i64())
        })
    {
        return Some(n > 0);
    }
    None
}

fn first_string(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            if let Some(s) = v.as_str() {
                let t = s.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

fn is_safe_key(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Parse catalog entries list payloads (`items` / bare array / nested).
pub fn parse_catalog_entries_value(value: &Value) -> Vec<CatalogEntryView> {
    let items = extract_items(value);
    let mut out = Vec::new();
    for item in items {
        if let Some(view) = parse_one_catalog_entry(item) {
            out.push(view);
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    out
}

fn parse_one_catalog_entry(item: &Value) -> Option<CatalogEntryView> {
    let obj = item.as_object()?;
    let id = first_string(obj, &["id", "name", "entryID", "entry_id"])
        .unwrap_or_else(|| "unknown".into());

    let manifest = obj.get("manifest").and_then(|v| v.as_object());
    let name = manifest
        .and_then(|m| first_string(m, &["name", "Name", "title", "Title"]))
        .or_else(|| first_string(obj, &["name", "displayName", "display_name", "title"]))
        .unwrap_or_else(|| id.clone());
    let short_description = manifest
        .and_then(|m| first_string(m, &["shortDescription", "short_description"]))
        .or_else(|| first_string(obj, &["shortDescription", "short_description"]))
        .unwrap_or_default();
    let description = manifest
        .and_then(|m| first_string(m, &["description", "Description"]))
        .or_else(|| first_string(obj, &["description"]))
        .unwrap_or_default();
    let user_type = manifest
        .and_then(|m| first_string(m, &["serverUserType", "server_user_type", "userType"]))
        .or_else(|| first_string(obj, &["serverUserType", "userType"]))
        .unwrap_or_default();
    let catalog_name =
        first_string(obj, &["catalogName", "catalog_name", "mcpCatalogName"]).unwrap_or_default();
    let connect_url =
        first_string(obj, &["connectURL", "connect_url", "connectUrl"]).unwrap_or_default();
    let oauth_configured = obj
        .get("oauthCredentialConfigured")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    Some(CatalogEntryView {
        id,
        name,
        short_description,
        description,
        user_type,
        catalog_name,
        connect_url,
        oauth_configured,
    })
}

/// Parse MCP Registry API list (`servers` array of `{ server, _meta }`).
pub fn parse_registry_servers_value(value: &Value) -> Vec<CatalogEntryView> {
    let servers = value
        .get("servers")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().collect::<Vec<_>>())
        .unwrap_or_else(|| extract_items(value));

    let mut out = Vec::new();
    for item in servers {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };
        // Nested under "server" for registry responses.
        let server = obj.get("server").and_then(|v| v.as_object()).unwrap_or(obj);
        let name =
            first_string(server, &["name", "title", "id"]).unwrap_or_else(|| "unknown".into());
        let title = first_string(server, &["title"]).unwrap_or_else(|| name.clone());
        let description = first_string(server, &["description"]).unwrap_or_default();
        let connect_url = server
            .get("remotes")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.as_object())
            .and_then(|r| first_string(r, &["url", "URL"]))
            .unwrap_or_default();
        let configuration_required = obj
            .get("_meta")
            .and_then(|m| m.get("ai.obot/server"))
            .and_then(|m| m.get("configurationRequired"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        out.push(CatalogEntryView {
            id: name.clone(),
            name: title,
            short_description: description.chars().take(120).collect(),
            description,
            user_type: String::new(),
            catalog_name: "registry".into(),
            connect_url,
            oauth_configured: !configuration_required,
        });
    }
    out.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    out
}

fn parse_create_server_result(value: &Value, alias: Option<&str>) -> Option<CreateServerResult> {
    let obj = value.as_object()?;
    let id = first_string(obj, &["id", "name"]).unwrap_or_default();
    let alias_field = first_string(obj, &["alias"]).unwrap_or_default();
    let manifest = obj.get("manifest").and_then(|v| v.as_object());
    let display_name = manifest
        .and_then(|m| first_string(m, &["name", "Name"]))
        .or_else(|| first_string(obj, &["alias", "name"]))
        .unwrap_or_else(|| {
            if !alias_field.is_empty() {
                alias_field.clone()
            } else {
                id.clone()
            }
        });
    let connect_url =
        first_string(obj, &["connectURL", "connect_url", "connectUrl"]).unwrap_or_default();
    let configured = obj
        .get("configured")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let missing_oauth = obj
        .get("missingOAuthCredentials")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let key = alias
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            if is_safe_key(&alias_field) {
                Some(alias_field)
            } else {
                None
            }
        })
        .or_else(|| {
            let slug = slugify(&display_name);
            if is_safe_key(&slug) {
                Some(slug)
            } else {
                None
            }
        })
        .unwrap_or_else(|| slugify(&id));

    Some(CreateServerResult {
        id,
        key,
        display_name,
        connect_url,
        configured,
        missing_oauth,
    })
}

/// Lowercase alphanumeric / `_` / `-` slug.
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if (c == '_' || c == '-' || c == '.' || c.is_whitespace())
            && !prev_dash
            && !out.is_empty()
        {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "server".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_config_defaults_when_missing() {
        let dir = TempDir::new().unwrap();
        let cfg = load_config(dir.path()).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.base_url, "http://127.0.0.1:8080");
        assert_eq!(cfg.open_admin_path, "/mcp-catalog");
    }

    #[test]
    fn load_config_from_file() {
        let dir = TempDir::new().unwrap();
        let path = crate::paths::obot_config_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
enabled = false
base_url = "http://localhost:9090"
bootstrap_token = "secret"
open_admin_path = "/admin"
"#,
        )
        .unwrap();
        let cfg = load_config(dir.path()).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.base_url, "http://localhost:9090");
        assert_eq!(cfg.bootstrap_token.as_deref(), Some("secret"));
        assert_eq!(cfg.admin_url(), "http://localhost:9090/admin");
    }

    #[test]
    fn parse_canonical_items_list() {
        let json = serde_json::json!({
            "items": [
                {
                    "id": "ms_abc",
                    "alias": "stripe",
                    "connectURL": "http://127.0.0.1:8080/mcp-connect/ms_abc",
                    "manifest": { "name": "Stripe" },
                    "missingOAuthCredentials": false,
                    "deploymentStatus": "Ready",
                    "deploymentReadyReplicas": 1
                },
                {
                    "id": "ms_def",
                    "connect_url": "http://127.0.0.1:8080/mcp-connect/ms_def",
                    "manifest": { "name": "Gmail Tools" },
                    "missingOAuthCredentials": true
                }
            ]
        });
        let servers = parse_servers_value(&json);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].key, "gmail-tools");
        assert_eq!(servers[1].key, "stripe");
        match &servers[1].source {
            ServerSource::ObotGateway { connect_url, .. } => {
                assert!(connect_url.contains("mcp-connect"));
            }
            _ => panic!("expected ObotGateway"),
        }
        assert_eq!(servers[1].oauth_ok, Some(true));
        assert_eq!(servers[1].running, Some(true));
        assert_eq!(servers[0].oauth_ok, Some(false));
    }

    #[test]
    fn parse_bare_array_and_url_field() {
        let json = serde_json::json!([
            { "id": "x1", "name": "Tool", "url": "http://host/mcp-connect/x1" }
        ]);
        let servers = parse_servers_value(&json);
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers[0].source.connect_url(),
            Some("http://host/mcp-connect/x1")
        );
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Gmail Tools"), "gmail-tools");
        assert_eq!(slugify("stripe"), "stripe");
        assert_eq!(slugify("  "), "server");
    }

    #[test]
    fn health_disabled() {
        let cfg = ObotConfig {
            enabled: false,
            ..ObotConfig::default()
        };
        let h = health_with_config(&cfg).unwrap();
        assert_eq!(h.status, ObotHealthStatus::Disabled);
    }

    #[test]
    fn parse_catalog_entries_items() {
        let json = serde_json::json!({
            "items": [
                {
                    "id": "github-entry",
                    "catalogName": "default",
                    "oauthCredentialConfigured": true,
                    "connectURL": "http://127.0.0.1:8080/mcp-connect/github-entry",
                    "manifest": {
                        "name": "GitHub",
                        "shortDescription": "GitHub tools",
                        "description": "Full GitHub MCP",
                        "serverUserType": "singleUser"
                    }
                },
                {
                    "id": "slack-entry",
                    "manifest": { "name": "Slack", "description": "Slack workspace" }
                }
            ]
        });
        let entries = parse_catalog_entries_value(&json);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "GitHub");
        assert_eq!(entries[0].id, "github-entry");
        assert_eq!(entries[0].short_description, "GitHub tools");
        assert_eq!(entries[0].user_type, "singleUser");
        assert_eq!(entries[1].name, "Slack");
    }

    #[test]
    fn filter_catalog_entries_by_query() {
        let mut entries = vec![
            CatalogEntryView {
                id: "a".into(),
                name: "GitHub".into(),
                short_description: "code".into(),
                description: "repos".into(),
                user_type: String::new(),
                catalog_name: "default".into(),
                connect_url: String::new(),
                oauth_configured: true,
            },
            CatalogEntryView {
                id: "b".into(),
                name: "Slack".into(),
                short_description: "chat".into(),
                description: "messages".into(),
                user_type: String::new(),
                catalog_name: "default".into(),
                connect_url: String::new(),
                oauth_configured: true,
            },
        ];
        filter_catalog_entries(&mut entries, "git");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "GitHub");
    }

    #[test]
    fn parse_registry_servers_list() {
        let json = serde_json::json!({
            "servers": [
                {
                    "server": {
                        "name": "io.github.example/weather",
                        "title": "Weather",
                        "description": "Forecasts",
                        "remotes": [{ "type": "streamable-http", "url": "http://host/mcp" }]
                    },
                    "_meta": { "ai.obot/server": { "configurationRequired": false } }
                }
            ]
        });
        let entries = parse_registry_servers_value(&json);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Weather");
        assert_eq!(entries[0].connect_url, "http://host/mcp");
    }

    #[test]
    fn parse_create_server_response() {
        let json = serde_json::json!({
            "id": "ms_abc",
            "alias": "github",
            "connectURL": "http://127.0.0.1:8080/mcp-connect/ms_abc",
            "configured": true,
            "missingOAuthCredentials": false,
            "manifest": { "name": "GitHub" }
        });
        let r = parse_create_server_result(&json, Some("github")).unwrap();
        assert_eq!(r.id, "ms_abc");
        assert_eq!(r.key, "github");
        assert_eq!(r.connect_url, "http://127.0.0.1:8080/mcp-connect/ms_abc");
        assert!(r.configured);
        assert!(!r.missing_oauth);
    }
}
