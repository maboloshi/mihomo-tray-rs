//! Paths that come from `tray.yml`.
//!
//! mihomo accepts relative paths for `-d` and `-f` and resolves them against its
//! own working directory. For a kernel this program starts, that directory is one
//! nobody chose — an Explorer launch hands the child whatever the tray inherited —
//! so the same relative path can name one file to this program and another to the
//! kernel. Such a path is refused here, and `%NAME%` expansion is offered instead:
//! it keeps a path machine independent without making it ambiguous.

use std::path::PathBuf;

use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;

use crate::i18n;

/// Expand `%NAME%` the way cmd expands it.
///
/// This is `ExpandEnvironmentStrings`, so the behaviour is the documented one:
/// names are case-insensitive, an unknown name is left as it is, and text that
/// does not form a name (`%`, `100%`) is untouched. Expanding here rather than
/// leaving it to a shell is what makes the command line this program builds the
/// same one a user would get by typing it in cmd.
pub fn expand(value: &str) -> String {
    let source: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // A length of zero is how this call reports a failure; it cannot happen
        // for a value that came out of a YAML scalar, and the value itself is the
        // honest fallback if it ever does.
        let needed = ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0);
        if needed == 0 {
            return value.to_string();
        }
        let mut buffer = vec![0u16; needed as usize];
        let written = ExpandEnvironmentStringsW(source.as_ptr(), buffer.as_mut_ptr(), needed);
        if written == 0 || written > needed {
            return value.to_string();
        }
        // `written` counts the terminating NUL.
        String::from_utf16_lossy(&buffer[..written as usize - 1])
    }
}

/// The absolute path `value` names, with `%NAME%` expanded first.
///
/// `field` is the setting the value came from (`mihomo.path`, …) and ends up in
/// the report: a path that cannot be used has to name the line to fix. Both
/// spellings are reported, because with `%NAME%` in the value the expanded one is
/// what tells a broken path from a path this program misread.
pub fn absolute(field: &str, value: &str) -> Result<PathBuf, String> {
    let expanded = expand(value);
    let path = PathBuf::from(&expanded);
    if !path.is_absolute() {
        return Err(i18n::t().error_path_not_absolute(field, value, &expanded));
    }
    Ok(path)
}

/// The absolute path of `value`, which has to name an existing file.
pub fn file(field: &str, value: &str) -> Result<PathBuf, String> {
    let path = absolute(field, value)?;
    if !path.is_file() {
        return Err(i18n::t().error_path_missing(field, &path.display().to_string()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_is_not_a_variable_survives() {
        // Same as cmd: nothing to expand is not an error.
        assert_eq!(
            expand("%MIHOMO_TRAY_NO_SUCH_VAR%\\x"),
            "%MIHOMO_TRAY_NO_SUCH_VAR%\\x"
        );
        assert_eq!(expand("100%"), "100%");
        assert_eq!(expand("plain"), "plain");
    }

    #[test]
    fn a_variable_is_expanded_whatever_its_case() {
        // `%USERPROFILE%` is the variable every Windows session has, and the one
        // the samples tell users to write paths with.
        let Some(profile) = std::env::var_os("USERPROFILE") else {
            return;
        };
        let profile = profile.to_string_lossy().into_owned();
        assert_eq!(
            expand("%USERPROFILE%\\mihomo"),
            format!("{profile}\\mihomo")
        );
        assert_eq!(
            expand("%userprofile%\\mihomo"),
            format!("{profile}\\mihomo")
        );
    }

    #[test]
    fn absolute_means_absolute() {
        assert!(absolute("f", r"C:\mihomo\config.yaml").is_ok());
        assert!(absolute("f", r"\\server\share\config.yaml").is_ok());
        assert!(absolute("f", r"%USERPROFILE%\config.yaml").is_ok());
        for value in [
            r"config.yaml",
            r".\config.yaml",
            r"\config.yaml",
            r"C:config.yaml",
            r"~\config.yaml",
            r"$HOME/config.yaml",
        ] {
            let error = absolute("f", value).unwrap_err();
            // The report names the setting, so the message points at the line.
            assert!(error.contains('f'), "{value}: {error}");
            assert!(error.contains(value), "{value}: {error}");
        }
    }

    #[test]
    fn a_file_has_to_exist() {
        let error = file("f", r"Z:\definitely\missing\config.yaml").unwrap_err();
        assert!(error.contains("Z:"), "{error}");
        let here = std::env::current_exe().expect("own image path");
        assert!(file("f", &here.display().to_string()).is_ok());
    }
}
