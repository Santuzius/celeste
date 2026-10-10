//! File names Android's shared storage refuses. It applies FAT's rules, so a remote file such as `https://ok.ru/g.txt` (rclone already turns its slashes into `／`) cannot be written there because of the colon. Like rclone's encoding for Windows, such characters get their full-width look-alikes on disk (`：`), and a look-alike that really is in a remote name is kept apart by a leading `‛`. Elsewhere names stay as they are.

use std::borrow::Cow;

/// Whether this platform's local names need the mapping.
const ACTIVE: bool = cfg!(target_os = "android");

/// Characters FAT forbids (besides `/`, which never reaches a name) and their look-alikes.
const MAP: [(char, char); 8] = [('"', '＂'), ('*', '＊'), (':', '：'), ('<', '＜'), ('>', '＞'), ('?', '？'), ('\\', '＼'), ('|', '｜')];

/// Marks a look-alike (or itself) that is part of the remote name.
const QUOTE: char = '‛';

/// The on-disk form of `remote` (a name or a `/`-separated relative path).
pub fn to_local(remote: &str) -> Cow<'_, str> {
    if ACTIVE { encode(remote) } else { Cow::Borrowed(remote) }
}

/// The remote form of the on-disk name `local`.
pub fn from_local(local: &str) -> Cow<'_, str> {
    if ACTIVE { decode(local) } else { Cow::Borrowed(local) }
}

fn encode(remote: &str) -> Cow<'_, str> {
    if !remote.chars().any(|c| c == QUOTE || MAP.iter().any(|&(plain, wide)| c == plain || c == wide)) {
        return Cow::Borrowed(remote);
    }
    let mut out = String::with_capacity(remote.len() + 8);
    let mut chars = remote.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(&(_, wide)) = MAP.iter().find(|&&(plain, _)| plain == c) {
            out.push(wide);
            continue;
        }
        // A look-alike, or a quote that would otherwise read as one, gets a quote of its own.
        let wide = MAP.iter().any(|&(_, wide)| wide == c);
        let quote_before_special = c == QUOTE && chars.peek().is_some_and(|&next| next == QUOTE || MAP.iter().any(|&(plain, wide)| next == plain || next == wide));
        if wide || quote_before_special {
            out.push(QUOTE);
        }
        out.push(c);
    }
    Cow::Owned(out)
}

fn decode(local: &str) -> Cow<'_, str> {
    if !local.chars().any(|c| c == QUOTE || MAP.iter().any(|&(_, wide)| c == wide)) {
        return Cow::Borrowed(local);
    }
    let mut out = String::with_capacity(local.len());
    let mut chars = local.chars().peekable();
    while let Some(c) = chars.next() {
        // Only a quote before a look-alike or another quote is one; elsewhere it is part of the name.
        if c == QUOTE
            && let Some(&next) = chars.peek()
            && (next == QUOTE || MAP.iter().any(|&(_, wide)| wide == next))
        {
            out.push(next);
            chars.next();
        } else if let Some(&(plain, _)) = MAP.iter().find(|&&(_, wide)| wide == c) {
            out.push(plain);
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn forbidden_characters_get_look_alikes_and_come_back() {
        for name in ["https:／／ok.ru／g.txt", "a\"b*c:d<e>f?g\\h|i", "plain.txt", "already：wide", "quote‛d", "‛：", "‛:", "‛‛", "dir:1/file?.txt"] {
            let local = encode(name);
            assert!(!local.chars().any(|c| "\"*:<>?\\|".contains(c)), "{local}");
            assert_eq!(decode(&local), name);
        }
        assert_eq!(encode("https:／／ok.ru／g.txt"), "https：／／ok.ru／g.txt");
        assert_eq!(encode("plain.txt"), "plain.txt");
        // A quote elsewhere is just part of the name, both ways.
        assert_eq!(decode("‛x"), "‛x");
        assert_eq!(encode("‛x"), "‛x");
    }
}
