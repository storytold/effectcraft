//! `Event::OpenUrl` for local files and folders: `file://` URLs open with the operating system
//! (Explorer, `open`, `xdg-open`), as `File.execute()` and Reveal in Explorer / Finder promise,
//! not in the web browser (which only downloads an executable or shows a folder listing). Other
//! URLs, and every URL in the browser build, still go to the browser.

/// Open `url`: a local `file://` URL with the operating system, anything else in the browser.
pub(crate) fn open(ctx: &egui::Context, url: String) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = local_path(&url)
        && launch(&path).is_ok()
    {
        return;
    }
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

/// The local path of a `file:///…` URL (percent-decoded), or `None` for other URLs, URLs with a
/// host and paths that don't decode to UTF-8.
pub(crate) fn local_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    if !rest.starts_with('/') {
        return None;
    }
    let decoded = percent_decode(rest)?;
    Some(without_drive_slash(decoded))
}

/// `file:///C:/…`: on Windows a drive path has no leading slash. Elsewhere `/C:/…` is an
/// ordinary absolute path and stays as it is.
#[cfg(target_os = "windows")]
fn without_drive_slash(path: String) -> String {
    let bytes = path.as_bytes();
    let drive = bytes.get(1).is_some_and(u8::is_ascii_alphabetic) && bytes.get(2) == Some(&b':');
    if drive { path.get(1..).unwrap_or_default().to_string() } else { path }
}

#[cfg(not(target_os = "windows"))]
fn without_drive_slash(path: String) -> String {
    path
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let high = bytes.get(i + 1).and_then(|c| char::from(*c).to_digit(16))?;
            let low = bytes.get(i + 2).and_then(|c| char::from(*c).to_digit(16))?;
            out.push(u8::try_from(high * 16 + low).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn launch(path: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("explorer.exe");
        c.arg(path.replace('/', "\\"));
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    let mut child = command.spawn()?;
    // Wait for the opener off the UI thread so it doesn't stay behind as a zombie on Unix. If the
    // thread can't be started the opener still runs; only its exit status goes unread.
    let _ = std::thread::Builder::new().name("open-local".into()).spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::local_path;

    #[test]
    fn file_urls_become_local_paths() {
        assert_eq!(local_path("file:///home/ann/a%23b%25c%3Fd.mov").as_deref(), Some("/home/ann/a#b%c?d.mov"));
        assert_eq!(local_path("file:///Users/ann/Caf%C3%A9/x.mov").as_deref(), Some("/Users/ann/Café/x.mov"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_paths_lose_the_leading_slash() {
        assert_eq!(local_path("file:///C:/Users/Ann/Footage/clip%201.mov").as_deref(), Some("C:/Users/Ann/Footage/clip 1.mov"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn drive_like_paths_stay_absolute_elsewhere() {
        assert_eq!(local_path("file:///C:/media/clip.mov").as_deref(), Some("/C:/media/clip.mov"));
    }

    #[test]
    fn other_urls_stay_with_the_browser() {
        assert_eq!(local_path("https://getartcraft.com"), None);
        assert_eq!(local_path("file://server/share/x.mov"), None);
        assert_eq!(local_path("file:///bad%2"), None, "a cut escape is not decoded");
        assert_eq!(local_path("file:///bad%zz"), None);
        assert_eq!(local_path("file:///bad%FF"), None, "not UTF-8");
    }
}
