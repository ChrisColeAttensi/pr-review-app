//! Author avatars for menu rows. Fetched once per login with curl, resized by `sips` to a
//! 36px PNG (18pt on Retina, the height a menu item icon is drawn at), and cached on disk.

use pr_core::github::Pr;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::image::Image;

fn dir(data: &Path) -> PathBuf {
    data.join("avatars")
}

fn file(data: &Path, login: &str) -> PathBuf {
    dir(data).join(format!("{login}.png"))
}

/// Fetch avatars not on disk yet. Returns true if any arrived. macOS only (needs `sips`).
pub fn fetch_missing(data: &Path, prs: &[Pr]) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let _ = std::fs::create_dir_all(dir(data));
    let mut seen = HashSet::new();
    let mut got = false;
    for p in prs {
        let login = p.author.as_str();
        // Logins are [A-Za-z0-9-] (bots add "[bot]"); anything else stays off the filesystem.
        if login.is_empty() || p.author_avatar.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || "-_[]".contains(c)) {
            continue;
        }
        if !seen.insert(login) || file(data, login).exists() {
            continue;
        }
        let sep = if p.author_avatar.contains('?') { '&' } else { '?' };
        let url = format!("{}{sep}s=72", p.author_avatar);
        let tmp = dir(data).join(format!("{login}.download"));
        let ok = std::process::Command::new("curl").args(["-sfL", "-m", "10", "-o"]).arg(&tmp).arg(&url).status().is_ok_and(|s| s.success())
            && std::process::Command::new("sips")
                .args(["-s", "format", "png", "-z", "36", "36"])
                .arg(&tmp)
                .arg("--out")
                .arg(file(data, login))
                .output()
                .is_ok_and(|o| o.status.success());
        let _ = std::fs::remove_file(&tmp);
        got |= ok;
    }
    got
}

/// The cached avatar, cut to a circle.
pub fn load(data: &Path, login: &str) -> Option<Image<'static>> {
    let bytes = std::fs::read(file(data, login)).ok()?;
    let img = Image::from_bytes(&bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let mut rgba = img.rgba().to_vec();
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, w.min(h) as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            // One pixel of soft edge so the circle isn't jagged.
            let a = (r - d).clamp(0.0, 1.0);
            let i = ((y * w + x) * 4 + 3) as usize;
            rgba[i] = (rgba[i] as f32 * a) as u8;
        }
    }
    Some(Image::new_owned(rgba, w, h))
}

/// Hover tooltips and avatars for menu rows, applied natively as the menu opens. Tauri's
/// menu API has no tooltips, and macOS 27 doesn't draw `NSMenuItem.image` in this menu, so
/// the avatar is moved inline into the title as a text attachment.
#[cfg(target_os = "macos")]
pub fn install_menu_hooks(app: tauri::AppHandle) {
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSAttributedStringAttachmentConveniences, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSMenu, NSMenuDidBeginTrackingNotification, NSMenuItem,
        NSTextAttachment,
    };
    use objc2_foundation::{NSAttributedString, NSDictionary, NSMutableAttributedString, NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize, NSString};
    use std::ptr::NonNull;
    use tauri::Manager;

    fn inline_avatar(item: &NSMenuItem) {
        let Some(image) = item.image() else { return };
        let title = item.title();
        let attachment = NSTextAttachment::new();
        attachment.setImage(Some(&image));
        // Sit the 18pt circle on the text's centre line rather than its baseline.
        attachment.setBounds(NSRect::new(NSPoint::new(0.0, -4.0), NSSize::new(18.0, 18.0)));
        let out = NSMutableAttributedString::new();
        out.appendAttributedString(&NSAttributedString::attributedStringWithAttachment(&attachment));
        let font = NSFont::menuFontOfSize(0.0);
        let color = NSColor::labelColor();
        let attrs = unsafe {
            NSDictionary::<NSString, AnyObject>::from_slices(
                &[NSFontAttributeName, NSForegroundColorAttributeName],
                &[&*font as &AnyObject, &*color as &AnyObject],
            )
        };
        let text = unsafe { NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &NSString::from_str(&format!("  {title}")), Some(&attrs)) };
        out.appendAttributedString(&text);
        item.setAttributedTitle(Some(&out));
        item.setImage(None);
    }

    let block = block2::RcBlock::new(move |n: NonNull<NSNotification>| {
        let n = unsafe { n.as_ref() };
        let Some(obj) = n.object() else { return };
        let Ok(menu) = obj.downcast::<NSMenu>() else { return };
        let state = app.state::<crate::BarState>();
        let tips = state.tooltips.lock().unwrap();
        for item in menu.itemArray().iter() {
            inline_avatar(&item);
            // An inlined title starts with the attachment character and the spacer.
            let title = item.title().to_string();
            let key = title.trim_start_matches(['\u{FFFC}', ' ']);
            if let Some(tip) = tips.get(key) {
                item.setToolTip(Some(&NSString::from_str(tip)));
            }
        }
    });
    unsafe {
        let center = NSNotificationCenter::defaultCenter();
        let token = center.addObserverForName_object_queue_usingBlock(Some(NSMenuDidBeginTrackingNotification), None, None, &block);
        // Lives for the whole app.
        std::mem::forget(token);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install_menu_hooks(_app: tauri::AppHandle) {}
