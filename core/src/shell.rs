//! Running `gh`, `git` and friends. A GUI app on macOS starts with a bare PATH,
//! so we borrow the user's login-shell PATH once at startup.

use std::path::Path;
use std::process::Command;

/// A `Command` that never flashes a console window when spawned from the GUI on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Windows GUI apps already inherit the user's full PATH; only add the native installer's dir.
#[cfg(windows)]
pub fn adopt_login_shell_path() {
    if let Some(home) = dirs::home_dir() {
        let extra = home.join(".local").join("bin");
        let mut paths: Vec<_> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        if !paths.contains(&extra) {
            paths.push(extra);
            if let Ok(joined) = std::env::join_paths(paths) {
                std::env::set_var("PATH", joined);
            }
        }
    }
}

#[cfg(not(windows))]
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
    let mut c = command(cmd);
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
    #[cfg(windows)]
    let found = run("where.exe", &[name], None).ok().and_then(|s| {
        // `where` lists every match in PATH order; a real .exe spawns more reliably
        // than an npm .cmd shim, which goes through cmd.exe's argument parsing.
        let lines: Vec<String> = s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        lines.iter().find(|l| l.to_lowercase().ends_with(".exe")).or(lines.first()).cloned()
    });
    #[cfg(not(windows))]
    let found = run("/usr/bin/which", &[name], None).ok().map(|s| s.trim().to_string());
    found.filter(|s| !s.is_empty())
}


/// Is this app installed? `mac_name` as `open -a` knows it; on Windows, by its URL scheme.
pub fn app_installed(mac_name: &str, scheme: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = scheme;
        Command::new("open").args(["-Ra", mac_name]).output().map(|o| o.status.success()).unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let _ = mac_name;
        command("reg").args(["query", &format!(r"HKCU\Software\Classes\{scheme}")]).output().map(|o| o.status.success()).unwrap_or(false)
            || command("reg").args(["query", &format!(r"HKCR\{scheme}")]).output().map(|o| o.status.success()).unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (mac_name, scheme);
        false
    }
}
