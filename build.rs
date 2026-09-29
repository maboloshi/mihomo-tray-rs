//! Build script.
//!
//! Two jobs, both about a file the repository ships and the binary embeds:
//!
//! * the Windows application manifest is embedded with plain linker arguments;
//! * `lang/en-US.yml` is rewritten from the built-in English table in
//!   `src/i18n.rs`, so the file a translator copies cannot drift from the
//!   strings the program actually falls back to.
//!
//! The language template is generated rather than hand-kept because its whole
//! content is already in the binary: the keys come from the `messages!` list and
//! the strings from the English table. The region below the marker line is
//! rewritten whenever the bytes would change, and the comment header above it is
//! left as it is written. The crate's
//! `shipped_template_is_regenerated_from_the_builtin_table` test compares the file
//! with the fingerprint of what this script built — that is what makes the
//! generator the only author of the body.

use std::path::{Path, PathBuf};

/// The built-in table the language template is generated from.
const I18N: &str = "src/i18n.rs";
/// The generated template, also the file a translator is told to copy.
const LANGUAGE_TEMPLATE: &str = "lang/en-US.yml";
/// The table slice to read: from here…
const TABLE_START: &str = "static EN_US";
/// …to here, which is what follows that table. Without an end, a later
/// `Cow::Borrowed` — the Chinese table is one — would be read as another entry of
/// the English one.
const TABLE_END: &str = "static MESSAGES";
/// The `field => "section.name"` list the whole table is declared from. It is
/// what turns a table field into the key the language file uses, and it is the
/// only place a key is spelled.
const MESSAGES_START: &str = "messages! {";
/// Where the hand-written part of the template ends and the generated part
/// starts. One line, because it is both the "do not edit below" sign and the
/// split the build script and the test use. Keep in step with the same line in
/// `lang/en-US.yml` and in `src/i18n.rs`.
const MARKER: &str = "# Everything below this line is written by build.rs from the built-in English table in src/i18n.rs";

/// The rest of that sign: the marker line is wrapped in the template as well.
const MARKER_TAIL: &str = ": change a string there, not here.\n";

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    generate_language_template();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    if !manifest.exists() {
        return;
    }
    // Embed the application manifest (Common-Controls v6, supportedOS, per-monitor
    // DPI) with plain linker arguments so no extra build dependency is needed.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}

/// Replace the generated part of [`LANGUAGE_TEMPLATE`] with the entries of the
/// built-in English table, and hand the test the fingerprint of what it wrote.
///
/// The fingerprint is what makes "the shipped template is the generated one" a
/// check instead of a promise: there is no second implementation of the
/// generator to drift from this one.
fn generate_language_template() {
    println!("cargo:rerun-if-changed={I18N}");
    println!("cargo:rerun-if-changed={LANGUAGE_TEMPLATE}");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let template = root.join(LANGUAGE_TEMPLATE);
    // An installation of the sources without the template cannot be repaired
    // from here, and a checkout that never touched it needs no write.
    let Ok(current) = std::fs::read_to_string(&template) else {
        return;
    };
    let Ok(source) = std::fs::read_to_string(root.join(I18N)) else {
        return;
    };
    let Some(entries) = english_table(&source) else {
        return;
    };
    let Some(header) = current.split_once(MARKER).map(|(header, _)| header) else {
        println!(
            "cargo:warning={LANGUAGE_TEMPLATE} has no generated-section marker; left as it is"
        );
        return;
    };

    let mut text = String::with_capacity(current.len() + 64);
    text.push_str(header);
    text.push_str(MARKER);
    text.push_str(MARKER_TAIL);
    // The sections are the key prefixes of the declaration list, in its order.
    let mut section = "";
    for (key, value) in &entries {
        let (prefix, name) = key
            .split_once('.')
            .expect("messages! declares `section.name` keys");
        if prefix != section {
            section = prefix;
            text.push('\n');
            text.push_str(prefix);
            text.push_str(":\n");
        }
        text.push_str("  ");
        text.push_str(name);
        text.push_str(": ");
        text.push_str(&yaml_value(value));
        text.push('\n');
    }

    // The test compares this fingerprint with the template in the tree: as long
    // as the build script runs first, "the file on disk is what I just made"
    // holds only when the file was made from the current table.
    println!(
        "cargo:rustc-env=LANGUAGE_TEMPLATE_FINGERPRINT={:016x}",
        fingerprint(&text)
    );
    if text != current {
        std::fs::write(&template, text).expect("write the language template");
    }
}

/// A 64-bit FNV-1a fingerprint, enough to notice a changed string here without
/// pulling a hash crate into the build for a file nothing attacks. Handing the
/// whole template to the crate through the environment would not do: rustc's
/// environment is one line per variable.
fn fingerprint(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The `(key, value)` pairs of the built-in English table, in declaration order.
///
/// `src/i18n.rs` is not parsed as Rust. The `messages!` list is what declares
/// every string and its key, and the tables give each field its text, so the
/// keys come from the list and the values from the English table. `None` means
/// either part was not found, and nothing is written.
fn english_table(source: &str) -> Option<Vec<(String, String)>> {
    let values = borrowed_strings(source, TABLE_START, TABLE_END)?;
    let declared = message_keys(source)?;
    let mut entries = Vec::with_capacity(declared.len());
    for (field, key) in declared {
        // A field of `messages!` with no string is a table that lost a line.
        let value = values.iter().find(|(name, _)| *name == field)?;
        entries.push((key, value.1.clone()));
    }
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

/// The `field => "section.name"` entries of the `messages!` list, in declaration
/// order. Only the right-hand side is the key the language file uses; the field
/// name is how the tables name the same string.
fn message_keys(source: &str) -> Option<Vec<(String, String)>> {
    // The list starts on the line after the macro's opening brace.
    let (_, rest) = source[source.find(MESSAGES_START)?..].split_once('\n')?;
    let mut keys = Vec::new();
    for line in rest.lines() {
        let line = line.trim();
        // The list's own section comments, and the blank lines between them.
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        // The list is a plain `field => "key",` table: anything else ends it.
        let Some((field, key)) = line.split_once("=>") else {
            break;
        };
        let key = key.trim();
        let key = key.strip_suffix(',').unwrap_or(key).trim();
        // Every key is a quoted `section.name`; the macro's closing brace is not.
        if !key.starts_with('"') || !key.ends_with('"') {
            break;
        }
        keys.push((field.trim().to_string(), key.trim_matches('"').to_string()));
    }
    if keys.is_empty() { None } else { Some(keys) }
}

/// Every string of the `Cow::Borrowed("…")` table between `start` and `end`, each
/// one paired with the `name:` nearest before its literal.
///
/// A literal may open on the line after its `Cow::Borrowed(` and a value may be
/// spliced over several lines with a backslash; both are resolved, because the
/// text the program shows is what the language file has to carry.
fn borrowed_strings(source: &str, start: &str, end: &str) -> Option<Vec<(String, String)>> {
    let from = source.find(start)?;
    // The end offset is relative to `from`, not to the file.
    let to = from + source[from..].find(end)?;
    let mut rest = &source[from..to];
    let mut found = Vec::new();
    while let Some(open) = rest.find("Cow::Borrowed(") {
        let (key, value) = rest.split_at(open);
        rest = &value["Cow::Borrowed(".len()..];
        let name = ident_before(key)?;
        let (text, tail) = string_literal(rest.trim_start())?;
        found.push((name.to_string(), text));
        rest = tail;
    }
    if found.is_empty() { None } else { Some(found) }
}

/// The identifier of the `key:` immediately before a value, ignoring what the
/// previous value and its separators left behind. A value that starts on the
/// next line puts that newline and its indentation in the way.
fn ident_before(text: &str) -> Option<&str> {
    text.trim_end()
        .strip_suffix(':')?
        .trim_end()
        .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .next()
        .filter(|name| !name.is_empty())
}

/// The text of the `"…"` literal `rest` starts with, and what follows it.
///
/// Escapes are resolved the way rustc resolves them, including the `\` at the
/// end of a line that splices the next line in: the value in the table is the
/// value the program shows, so the generated YAML has to be the resolved one.
fn string_literal(after: &str) -> Option<(String, &str)> {
    let rest = after.strip_prefix('"')?;
    let mut text = String::new();
    let mut chars = rest.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((text, &rest[at + 1..])),
            '\\' => {
                let (next, escape) = chars.next()?;
                match escape {
                    // `\` at the end of a line splices the next line in, without
                    // its leading whitespace: continue after the indentation,
                    // with the same literal still open.
                    '\n' | '\r' => {
                        let mut after = next + escape.len_utf8();
                        if escape == '\r' && rest[after..].starts_with('\n') {
                            after += 1;
                        }
                        let indented = rest[after..].trim_start_matches([' ', '\t']);
                        let indent = rest[after..].len() - indented.len();
                        chars = rest[after..].char_indices();
                        for _ in 0..indent {
                            chars.next();
                        }
                    }
                    'n' => text.push('\n'),
                    'r' => text.push('\r'),
                    't' => text.push('\t'),
                    '0' => text.push('\0'),
                    '\\' => text.push('\\'),
                    '\'' => text.push('\''),
                    '"' => text.push('"'),
                    other => text.push(other),
                }
            }
            _ => text.push(c),
        }
    }
    None
}

/// One entry as a YAML line value.
///
/// The language reader takes everything after the first `:`, unquoted and
/// trimmed, so a value is quoted when losing its edges or taking its quotes for
/// delimiters would change it; anything else is written as it stands, because a
/// translator reads and copies these lines.
fn yaml_value(value: &str) -> String {
    let trimmed = value.trim();
    let needs_quotes = trimmed.len() != value.len()
        || value.starts_with(['"', '\''])
        || value.ends_with(['"', '\''])
        || value.contains(['\n', '\r', '\t'])
        || value.contains(": ")
        || value.ends_with(':')
        || value.contains('\\');
    if !needs_quotes {
        return value.to_string();
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for c in value.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}
