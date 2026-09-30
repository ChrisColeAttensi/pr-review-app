//! Running `gh`, `git` and friends. A GUI app on macOS starts with a bare PATH,
//! so we borrow the user's login-shell PATH once at startup.

use std::path::Path;
use std::process::Command;

pub fn adopt_login_shell_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = Command::new(&shell)
        .args(["-ilc", "printf '__PATH__%s__END__' \"$PATH\""])
        .output();
    if let Ok(out) = out {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(start) = text.find("__PATH__") {
            let rest = &text[start + 8..];
            if let Some(end) = rest.find("__END__") {
                let path = &rest[..end];
                if !path.is_empty() {
                    std::env::set_var("PATH", path);
                }
            }
        }
    }
    // Belt and braces for the usual suspects.
    if let Some(home) = dirs::home_dir() {
        let extra = [
            home.join(".local/bin"),
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
        ];
        let mut path = std::env::var("PATH").unwrap_or_default();
        for p in extra {
            let s = p.to_string_lossy().to_string();
            if !path.split(':').any(|x| x == s) {
                path = format!("{path}:{s}");
            }
        }
        std::env::set_var("PATH", path);
    }
}

pub fn run(cmd: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    let out = c.output().map_err(|e| format!("failed to run {cmd}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("{cmd} {}: {err}", args.first().unwrap_or(&"")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn which(name: &str) -> Option<String> {
    run("/usr/bin/which", &[name], None)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
