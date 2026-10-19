//! URI ↔ path conversion for the two client kinds.
//!
//! LSP addresses documents by URI; the design database keys everything by
//! `PathBuf`.  The two have to meet exactly, because [`DesignDb`] matches files
//! by path equality: the path the editor's `didOpen` produces must be the same
//! spelling the database used for `add_source`.
//!
//! Two families are supported:
//!
//! * `file:` URIs (desktop, and monaco with a real file system) are decoded to
//!   an absolute path; percent escapes are expanded, a leading `/` in front of
//!   a drive letter is dropped, drive letters are upper-cased and `/` becomes
//!   the platform separator.
//! * everything else (`inmemory://model/1`, `untitled:Untitled-1`) has no file
//!   system meaning.  Those URIs are used *verbatim* as the database key, which
//!   keeps them unique, round-trips them through [`to_uri`], and can never
//!   collide with a real path because a real path never contains `://`.
//!
//! [`DesignDb`]: crate::design::DesignDb

use std::path::{Path, PathBuf};

/// The database key for the document behind `uri`.
pub fn to_path(uri: &str) -> PathBuf {
    let Some((scheme, rest)) = split_scheme(uri) else {
        return PathBuf::from(uri);
    };
    if !scheme.eq_ignore_ascii_case("file") {
        // A virtual document: the URI itself is the key (see the module docs).
        return PathBuf::from(uri);
    }
    PathBuf::from(file_uri_to_path(rest))
}

/// The database key for a path a host loaded itself (a command line argument,
/// a file dialog selection).
///
/// `didOpen` keys documents by their URI, which is always absolute; a host that
/// adds sources through [`DesignDb::add_file`](crate::design::DesignDb::add_file)
/// has to use the same spelling, otherwise the same file ends up as two
/// differently keyed copies and its modules collide as duplicates.  This makes
/// the path absolute (relative to the process working directory) and applies
/// the same Windows drive-letter normalisation as [`to_path`].
///
/// A path that is already absolute is returned unchanged; a host without a
/// working directory (wasm) gets its input back.
pub fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if !cfg!(windows) {
        return absolute;
    }
    let text = absolute.to_string_lossy();
    let mut characters = text.chars();
    if let (Some(first), Some(':')) = (characters.next(), characters.next()) {
        if first.is_ascii_lowercase() {
            return PathBuf::from(format!("{}{}", first.to_ascii_uppercase(), &text[1..]));
        }
    }
    PathBuf::from(text.into_owned())
}

/// A `file:` URI for a database path (or the opaque key unchanged).
pub fn to_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.contains("://") {
        // An opaque key such as `inmemory://model/1`; it was a URI to begin with.
        return text.into_owned();
    }
    let mut normalised = text.replace('\\', "/");
    if !normalised.starts_with('/') {
        normalised.insert(0, '/');
    }
    format!("file://{}", percent_encode(&normalised))
}

/// `scheme`, `rest` (everything after the first `:`).
fn split_scheme(uri: &str) -> Option<(&str, &str)> {
    let colon = uri.find(':')?;
    let scheme = &uri[..colon];
    let valid = !scheme.is_empty()
        && scheme.starts_with(|character: char| character.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'));
    if !valid {
        return None;
    }
    Some((scheme, &uri[colon + 1..]))
}

fn file_uri_to_path(rest: &str) -> String {
    // `file:/abs/path`, `file:///abs/path` and `file://authority/path` all end
    // up with `rest` after the scheme; drop the two slashes of the authority
    // separator and keep whatever path follows.
    let without_slashes = rest.strip_prefix("//").unwrap_or(rest);
    let (authority, tail) = match without_slashes.find('/') {
        Some(slash) => (&without_slashes[..slash], &without_slashes[slash..]),
        None => (without_slashes, ""),
    };
    let decoded = percent_decode(tail);
    let mut path = decoded;

    if !authority.is_empty() {
        // `file://server/share/dir` is a UNC path on Windows.
        path = format!("//{authority}{path}");
    }

    if cfg!(windows) {
        path = path.replace('/', "\\");
        // `/C:\dir` -> `C:\dir`
        let bytes = path.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'\\' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            path.remove(0);
        }
        // Upper-case the drive letter so `file:///c:/x` and `C:\x` are the same
        // key (`Path` comparison is component-wise and case sensitive).
        let mut chars = path.chars();
        if let (Some(first), Some(':')) = (chars.next(), chars.next()) {
            if first.is_ascii_lowercase() {
                path = format!("{}{}", first.to_ascii_uppercase(), &path[1..]);
            }
        }
    }
    path
}

/// Expand `%XX` escapes.  Invalid escapes are kept verbatim, so a URI that
/// merely contains a `%` still maps to a stable key.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            match (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high * 16 + low);
                    index += 3;
                    continue;
                }
                _ => {}
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|error| {
        // A path with invalid UTF-8 cannot be produced from a URI anyway; fall
        // back to lossy decoding rather than dropping the document.
        String::from_utf8_lossy(error.as_bytes()).into_owned()
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-encode everything that is not safe in a path segment or separator.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let safe = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':' | b'@' | b'+' | b'$' | b',' | b';' | b'=');
        if safe {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(hex_digit(byte >> 4)));
            out.push(char::from(hex_digit(byte & 0x0f)));
        }
    }
    out
}

fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'A' + (value - 10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_map_to_the_same_path_spelling_the_design_db_uses() {
        let path = PathBuf::from(if cfg!(windows) {
            r"C:\project dir\rtl\top.sv"
        } else {
            "/project dir/rtl/top.sv"
        });
        let uri = to_uri(&path);
        assert!(uri.starts_with("file:///"), "{uri}");
        assert!(uri.contains("project%20dir"), "{uri}");
        assert_eq!(to_path(&uri), path);
    }

    #[test]
    fn file_uris_are_normalised_across_spellings() {
        // Non-file schemes keep the whole URI as the key.
        assert_eq!(
            to_path("inmemory://model/1"),
            PathBuf::from("inmemory://model/1")
        );
        assert_eq!(to_uri(Path::new("inmemory://model/1")), "inmemory://model/1");

        // Percent escapes and UTF-8 survive the round trip.
        let decoded = to_path("file:///rtl/%C3%A9t%C3%A9/a%20b.sv");
        let expected = PathBuf::from(if cfg!(windows) {
            r"\rtl\été\a b.sv"
        } else {
            "/rtl/été/a b.sv"
        });
        assert_eq!(decoded, expected);
        assert_eq!(to_path(&to_uri(&decoded)), decoded);
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_uris_share_one_key_per_file() {
        let with_slashes = to_path("file:///C:/rtl/top.sv");
        assert_eq!(with_slashes, PathBuf::from(r"C:\rtl\top.sv"));
        // Both separators and any drive-letter case are the same document.
        assert_eq!(to_path(r"file:///C:\rtl\top.sv"), with_slashes);
        assert_eq!(to_path("file:///c:/rtl/top.sv"), with_slashes);
        // `file:///C:/...` and the plain path a preload would use are equal.
        assert_eq!(with_slashes, PathBuf::from("C:/rtl/top.sv"));
    }

    #[test]
    fn malformed_percent_escapes_are_kept_verbatim() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%41%42"), "AB");
    }

    #[test]
    fn uris_without_a_scheme_stay_plain_paths() {
        assert_eq!(to_path("notes.sv"), PathBuf::from("notes.sv"));
        assert_eq!(to_path("C:/rtl/top.sv"), PathBuf::from("C:/rtl/top.sv"));
    }

    #[test]
    fn normalising_a_host_path_gives_an_absolute_database_key() {
        let normalized = normalize_path("rtl/top.sv");
        assert!(normalized.is_absolute(), "{normalized:?}");
        assert!(normalized.ends_with("rtl/top.sv") || normalized.ends_with(r"rtl\top.sv"));
        // Idempotent, so a host can normalise twice without a second key.
        assert_eq!(normalize_path(&normalized), normalized);
        // The URI the editor produces and the normalised host path agree.
        assert_eq!(to_path(&to_uri(&normalized)), normalized);
    }
}
