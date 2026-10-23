//! Windows notifications. A desktop app needs an app id that Windows knows
//! before its notifications show under its own name, so freezetime registers
//! one for the current user (HKCU, no admin rights) with its name and icon.

#[cfg(windows)]
const APP_ID: &str = "app.freezetime";

#[cfg(windows)]
pub fn init(icon_png: &[u8], data_dir: &std::path::Path) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let icon = data_dir.join("icon.png");
    if std::fs::read(&icon).ok().as_deref() != Some(icon_png) {
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::write(&icon, icon_png);
    }
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}")) {
        let _ = key.set_value("DisplayName", &"freezetime");
        let _ = key.set_value("IconUri", &icon.display().to_string());
    }
    unsafe {
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&windows::core::HSTRING::from(APP_ID));
    }
}

#[cfg(windows)]
pub fn show(title: &str, body: &str, sound: bool) {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }
    let audio = if sound { r#"<audio src="ms-winsoundevent:Notification.Default"/>"# } else { r#"<audio silent="true"/>"# };
    let xml = format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>{audio}</toast>"#,
        esc(title),
        esc(body)
    );
    let result = (|| -> windows::core::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)
    })();
    if let Err(e) = result {
        eprintln!("freezetime: couldn't show a notification: {e}");
    }
}

#[cfg(not(windows))]
pub fn init(_icon_png: &[u8], _data_dir: &std::path::Path) {}

#[cfg(not(windows))]
pub fn show(title: &str, body: &str, _sound: bool) {
    eprintln!("freezetime: {title}: {body}");
}
