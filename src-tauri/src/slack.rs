//! Slack DMs through the user's own Claude Slack connector (claude.ai Slack), so no
//! Slack app or admin approval is needed and messages come from the user.
//! Each call is a tiny `claude -p` on Haiku that may use one Slack tool and nothing else.

use crate::jobs::{claude_bin, claude_command};
use crate::settings::Settings;
use crate::shell;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;

#[derive(Clone)]
struct Tools {
    search: String,
    send: String,
}

static TOOLS: Mutex<Option<Tools>> = Mutex::new(None);

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SlackUser {
    pub user_id: String,
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lookup {
    pub user: Option<SlackUser>,
    /// Remembered from an earlier message, not looked up now.
    pub cached: bool,
    pub emails: Vec<String>,
}

fn valid_user_id(id: &str) -> bool {
    id.len() >= 8 && id.starts_with(['U', 'W']) && id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Claude Code names MCP tools `mcp__<server>__<tool>`, with the server name's
/// punctuation and spaces turned into `_` ("claude.ai Slack" → "claude_ai_Slack").
fn normalize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

fn tools_for(server: &str) -> Tools {
    let n = normalize(server);
    Tools { search: format!("mcp__{n}__slack_search_users"), send: format!("mcp__{n}__slack_send_message") }
}

/// Find a connected Slack MCP server via `claude mcp list`. Reading tool names from a
/// session's init event is unreliable: claude.ai connectors can still be connecting.
fn tools(settings: &Settings) -> Result<Tools, String> {
    if let Some(t) = TOOLS.lock().unwrap().clone() {
        return Ok(t);
    }
    let claude = claude_bin(settings)?;
    let out = claude_command(&claude)
        .args(["mcp", "list"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("failed to run claude mcp list: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Lines look like `claude.ai Slack: https://mcp.slack.com/mcp - ✔ Connected`.
    let mut servers: Vec<(String, bool)> = text
        .lines()
        .filter(|l| l.contains("mcp.slack.com"))
        .filter_map(|l| {
            let name = l.split(": http").next()?.trim().to_string();
            Some((name, l.contains("Connected") && !l.contains("Needs")))
        })
        .collect();
    // Prefer the claude.ai connector, then any other connected Slack server.
    servers.sort_by_key(|(n, _)| !n.starts_with("claude.ai"));
    let Some((name, _)) = servers.iter().find(|(_, ok)| *ok) else {
        return Err(if servers.is_empty() {
            NO_CONNECTOR.to_string()
        } else {
            format!(
                "Slack is set up for the claude CLI but not connected ({}). Reconnect it at claude.ai → Settings → Connectors.",
                servers.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
            )
        });
    };
    let t = tools_for(name);
    *TOOLS.lock().unwrap() = Some(t.clone());
    Ok(t)
}

const NO_CONNECTOR: &str =
    "The claude CLI has no Slack connector. Connect Slack at claude.ai → Settings → Connectors, then check `claude mcp list`.";

fn run_once(settings: &Settings, prompt: &str, allowed: &str, schema: &Value) -> Result<Value, String> {
    let claude = claude_bin(settings)?;
    let out = claude_command(&claude)
        .args([
            "-p", prompt,
            "--model", "haiku",
            "--output-format", "json",
            "--json-schema", &schema.to_string(),
            "--allowedTools", allowed,
            "--permission-mode", "dontAsk",
            "--no-session-persistence",
        ])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("failed to start claude: {e}"))?;
    serde_json::from_slice(&out.stdout)
        .map_err(|_| format!("claude returned no result. {}", String::from_utf8_lossy(&out.stderr).trim()))
}

/// One Haiku turn allowed to call only `allowed`, returning schema-shaped JSON.
/// If the tool name was guessed wrong, the denied call names the real one: learn it
/// and retry once.
fn run(settings: &Settings, prompt: &str, allowed: &str, schema: Value) -> Result<Value, String> {
    let suffix = allowed.rsplit("__").next().unwrap_or(allowed).to_string();
    let mut allowed = allowed.to_string();
    let mut prompt = prompt.to_string();
    for attempt in 0..2 {
        let v = run_once(settings, &prompt, &allowed, &schema)?;
        let denied: Vec<String> = v
            .get("permission_denials")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|x| x.get("tool_name")?.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if let Some(real) = denied.iter().find(|n| n.starts_with("mcp__") && n.ends_with(&format!("__{suffix}")) && **n != allowed) {
            if attempt == 0 {
                let server = real.trim_start_matches("mcp__").rsplit_once("__").map(|(s, _)| s.to_string()).unwrap_or_default();
                let t = Tools {
                    search: format!("mcp__{server}__slack_search_users"),
                    send: format!("mcp__{server}__slack_send_message"),
                };
                *TOOLS.lock().unwrap() = Some(t);
                prompt = prompt.replace(&allowed, real);
                allowed = real.clone();
                continue;
            }
        }
        if v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) {
            return Err(v.get("result").and_then(|r| r.as_str()).unwrap_or("claude reported an error").to_string());
        }
        return v
            .get("structured_output")
            .cloned()
            .filter(|s| s.is_object())
            .ok_or_else(|| "claude returned no structured output".into());
    }
    Err("could not reach the Slack tools".into())
}

fn cache_path(data: &Path) -> std::path::PathBuf {
    data.join("slack-users.json")
}

fn load_cache(data: &Path) -> HashMap<String, SlackUser> {
    std::fs::read_to_string(cache_path(data)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Work emails the author committed this PR with. Noreply addresses can't be matched.
fn author_emails(repo: &str, number: u64, login: &str) -> Vec<String> {
    let jq = format!(".[] | select(.author.login == \"{login}\") | .commit.author.email");
    let out = shell::run("gh", &["api", "--paginate", &format!("repos/{repo}/pulls/{number}/commits"), "--jq", &jq], None)
        .unwrap_or_default();
    let mut emails: Vec<String> = out
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|e| e.contains('@') && !e.ends_with("users.noreply.github.com"))
        .collect();
    emails.sort();
    emails.dedup();
    emails
}

pub fn lookup(settings: &Settings, data: &Path, repo: &str, number: u64, login: &str) -> Result<Lookup, String> {
    if let Some(u) = load_cache(data).get(login) {
        return Ok(Lookup { user: Some(u.clone()), cached: true, emails: vec![] });
    }
    let t = tools(settings)?;
    let emails = author_emails(repo, number, login);
    let name = shell::run("gh", &["api", &format!("users/{login}"), "--jq", ".name // \"\""], None)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if emails.is_empty() && name.is_empty() {
        return Ok(Lookup { user: None, cached: false, emails });
    }
    let prompt = format!(
"Find one Slack user. Do not send or change anything.
Call `{tool}` with `query` set to each email below in turn, stopping at the first exact email match: {emails:?}.
{name_line}
Return found=true only for an exact email match, or a single unambiguous full-name match. Otherwise found=false.",
        tool = t.search,
        name_line = if name.is_empty() { String::new() } else {
            format!("If no email matches, search once with `query` set to the full name \"{name}\".")
        },
    );
    let schema = json!({
        "type": "object",
        "required": ["found"],
        "properties": {
            "found": { "type": "boolean" },
            "userId": { "type": "string" },
            "name": { "type": "string" },
            "title": { "type": "string" },
            "email": { "type": "string" }
        }
    });
    let v = run(settings, &prompt, &t.search, schema)?;
    let user = (v.get("found").and_then(|f| f.as_bool()) == Some(true))
        .then(|| serde_json::from_value::<SlackUser>(v.clone()).ok())
        .flatten()
        .filter(|u| valid_user_id(&u.user_id));
    Ok(Lookup { user, cached: false, emails })
}

pub fn send(settings: &Settings, data: &Path, login: &str, user: &SlackUser, message: &str) -> Result<String, String> {
    if !valid_user_id(&user.user_id) {
        return Err(format!("not a Slack user id: {}", user.user_id));
    }
    if message.trim().is_empty() {
        return Err("empty message".into());
    }
    let t = tools(settings)?;
    let prompt = format!(
"Send exactly one Slack direct message, then stop.
Call `{tool}` once with channel_id=\"{id}\" and message set to the text between the markers, verbatim: do not add, remove, fix or reformat anything.
Return sent=true and the message link if the call succeeded.
<<<MESSAGE
{message}
MESSAGE>>>",
        tool = t.send,
        id = user.user_id,
    );
    let schema = json!({
        "type": "object",
        "required": ["sent"],
        "properties": { "sent": { "type": "boolean" }, "link": { "type": "string" }, "error": { "type": "string" } }
    });
    let v = run(settings, &prompt, &t.send, schema)?;
    if v.get("sent").and_then(|b| b.as_bool()) != Some(true) {
        return Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("Slack did not confirm the message").to_string());
    }
    // Confirmed by the user and delivered: remember the match.
    let mut cache = load_cache(data);
    cache.insert(login.to_string(), user.clone());
    let _ = std::fs::write(cache_path(data), serde_json::to_string_pretty(&cache).unwrap_or_default());
    Ok(v.get("link").and_then(|l| l.as_str()).unwrap_or_default().to_string())
}

/// Checks the whole path (CLI login, connector, tool names, sending) by DMing yourself.
pub fn send_test(settings: &Settings) -> Result<String, String> {
    let t = tools(settings)?;
    let prompt = format!(
"Send exactly one Slack direct message to the logged-in user themself (their own user id is given in the tool's description), then stop.
Call `{tool}` once with that user id as channel_id and message \"✅ PR Review can send Slack messages as you.\"
Return sent=true and the message link if the call succeeded.",
        tool = t.send,
    );
    let schema = json!({
        "type": "object",
        "required": ["sent"],
        "properties": { "sent": { "type": "boolean" }, "link": { "type": "string" }, "error": { "type": "string" } }
    });
    let v = run(settings, &prompt, &t.send, schema)?;
    if v.get("sent").and_then(|b| b.as_bool()) != Some(true) {
        return Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("Slack did not confirm the message").to_string());
    }
    Ok(v.get("link").and_then(|l| l.as_str()).unwrap_or_default().to_string())
}

pub fn forget(data: &Path, login: &str) {
    let mut cache = load_cache(data);
    if cache.remove(login).is_some() {
        let _ = std::fs::write(cache_path(data), serde_json::to_string_pretty(&cache).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_tools_like_claude_code() {
        let t = tools_for("claude.ai Slack");
        assert_eq!(t.send, "mcp__claude_ai_Slack__slack_send_message");
        assert_eq!(tools_for("plugin:engineering:slack").search, "mcp__plugin_engineering_slack__slack_search_users");
        assert!(valid_user_id("U01045HJK8X"));
        assert!(!valid_user_id("C123"));
        assert!(!valid_user_id("u01045hjk8x"));
    }
}
