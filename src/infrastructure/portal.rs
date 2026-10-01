//! Folder chooser through the XDG desktop portal (`org.freedesktop.portal.FileChooser`), so the dialog is the desktop's own (KDE, GNOME, …) without linking a GUI toolkit. Blocking — call from `spawn_blocking`.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use dbus::{
    arg::{PropMap, RefArg, Variant},
    blocking::Connection,
    message::MatchRule,
    Path,
};

/// How long we wait for the user to pick something before giving up.
const PICK_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Ask the user for a directory. `None` when cancelled or when no portal is available.
pub fn pick_folder(title: &str) -> Option<String> {
    match try_pick_folder(title) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("celeste: folder chooser unavailable ({err}).");
            None
        }
    }
}

fn try_pick_folder(title: &str) -> Result<Option<String>, dbus::Error> {
    let conn = Connection::new_session()?;
    // The portal answers on a request object whose path is derived from our bus name and a token we choose; subscribe before calling so the response can't race us.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let token = format!("celeste_{}_{nanos}", std::process::id());
    let sender = conn.unique_name().trim_start_matches(':').replace('.', "_");
    let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let rule = MatchRule::new_signal("org.freedesktop.portal.Request", "Response")
        .with_path(Path::new(request_path).map_err(|e| dbus::Error::new_failed(&e))?);

    let answer: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
    let slot = answer.clone();
    conn.add_match(rule, move |(code, results): (u32, PropMap), _, _| {
        let path = (code == 0).then(|| first_uri(&results)).flatten().and_then(|uri| file_uri_to_path(&uri));
        *slot.lock().unwrap() = Some(path);
        false
    })?;

    let mut options = PropMap::new();
    options.insert("handle_token".into(), Variant(Box::new(token)));
    options.insert("directory".into(), Variant(Box::new(true)));
    options.insert("modal".into(), Variant(Box::new(true)));
    let proxy = conn.with_proxy("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", Duration::from_secs(10));
    let _: (Path,) = proxy.method_call("org.freedesktop.portal.FileChooser", "OpenFile", ("", title, options))?;

    let deadline = Instant::now() + PICK_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(path) = answer.lock().unwrap().take() {
            return Ok(path);
        }
        conn.process(Duration::from_millis(500))?;
    }
    Ok(None)
}

fn first_uri(results: &PropMap) -> Option<String> {
    let mut uris = results.get("uris")?.0.as_iter()?;
    uris.next()?.as_str().map(str::to_owned)
}

/// `file:///home/me/My%20Docs` → `/home/me/My Docs`. Non-`file` URIs (e.g. a remote mount) are rejected.
fn file_uri_to_path(uri: &str) -> Option<String> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = encoded.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::file_uri_to_path;

    #[test]
    fn decodes_file_uris() {
        assert_eq!(file_uri_to_path("file:///home/me/My%20Docs").as_deref(), Some("/home/me/My Docs"));
        assert_eq!(file_uri_to_path("file:///home/me/AppData%F0%9F%93%B1").as_deref(), Some("/home/me/AppData📱"));
        assert_eq!(file_uri_to_path("sftp://host/x"), None);
    }
}
