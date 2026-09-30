//! Slack DMs through the user's own Claude Slack connector (claude.ai Slack), so no
//! Slack app or admin approval is needed and messages come from the user.
//! Each call is a tiny `claude -p` on Haiku that may use one Slack tool and nothing else.

use crate::jobs::{claude_bin, claude_command};
use crate::settings::Settings;
use crate::shell;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
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

/// MCP tool names depend on how the connector is registered, so read them from the
/// session's init event, then kill the process before it makes a model call.
fn tools(settings: &Settings) -> Result<Tools, String> {
    if let Some(t) = TOOLS.lock().unwrap().clone() {
        return Ok(t);
    }
    let claude = claude_bin(settings)?;
    let mut child = claude_command(&claude)
        .args(["-p", ".", "--output-format", "stream-json", "--verbose", "--model", "haiku",
               "--permission-mode", "dontAsk", "--no-session-persistence"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("failed to start claude: {e}"))?;
    let mut first = String::new();
    if let Some(out) = child.stdout.take() {
        let _ = BufReader::new(out).read_line(&mut first);
    }
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    let _ = child.wait();

    let init: Value = serde_json::from_str(&first).map_err(|_| "claude did not start a session (is the CLI logged in?)")?;
    let names: Vec<String> = init
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let find = |suffix: &str| names.iter().find(|n| n.starts_with("mcp__") && n.ends_with(suffix)).cloned();
    let t = Tools {
        search: find("__slack_search_users").ok_or(NO_CONNECTOR)?,
        send: find("__slack_send_message").ok_or(NO_CONNECTOR)?,
    };
    *TOOLS.lock().unwrap() = Some(t.clone());
    Ok(t)
}

const NO_CONNECTOR: &str =
    "The claude CLI has no Slack connector. Connect Slack at claude.ai → Settings → Connectors, then check `claude mcp list`.";

/// One Haiku turn allowed to call only `allowed`, returning schema-shaped JSON.
fn run(settings: &Settings, prompt: &str, allowed: &str, schema: Value) -> Result<Value, String> {
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
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| format!("claude returned no result. {}", String::from_utf8_lossy(&out.stderr).trim()))?;
    if v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) {
        return Err(v.get("result").and_then(|r| r.as_str()).unwrap_or("claude reported an error").to_string());
    }
    v.get("structured_output").cloned().filter(|s| s.is_object()).ok_or_else(|| "claude returned no structured output".into())
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
