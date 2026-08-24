/// Normalize a name for fuzzy matching: lowercase alphanumeric only.
pub fn slug(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

const BUNDLE_ROOTS: &[&str] = &[
    "com", "org", "net", "io", "dev", "app", "eu", "me", "uk", "de", "fr", "edu", "gov", "co",
    "tv", "cc", "info", "biz", "ai", "im", "us", "au", "jp", "ch", "nl", "se", "it", "es", "ca",
    "xyz",
];

pub fn looks_like_bundle_id(s: &str) -> bool {
    let s = s.trim().trim_end_matches(".plist");
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 3 {
        return false;
    }
    let first = parts[0].to_ascii_lowercase();
    BUNDLE_ROOTS.contains(&first.as_str())
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// `TEAMID.com.foo.bar` → `com.foo.bar`; `com.foo.bar.plist` → `com.foo.bar`.
pub fn extract_bundle_id(name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches(".plist");
    if looks_like_bundle_id(name) {
        return Some(name.to_string());
    }
    // Group Containers: 10+ alnum prefix, then a bundle id.
    if let Some((prefix, rest)) = name.split_once('.')
        && prefix.len() >= 8
        && prefix.chars().all(|c| c.is_ascii_alphanumeric())
        && looks_like_bundle_id(rest)
    {
        return Some(rest.to_string());
    }
    None
}

pub fn last_component(id: &str) -> &str {
    id.rsplit('.').next().unwrap_or(id)
}

pub fn vendor_prefix(id: &str) -> Option<String> {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() >= 2 {
        Some(format!("{}.{}", parts[0], parts[1]))
    } else {
        None
    }
}

pub fn is_apple_id(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.starts_with("com.apple.")
        || id.starts_with("com.apple")
        || id == "apple"
        || slug(&id).starts_with("comapple")
}

const GENERIC_TOKENS: &[&str] = &[
    "software",
    "app",
    "application",
    "applications",
    "inc",
    "ltd",
    "llc",
    "gmbh",
    "sas",
    "corp",
    "corporation",
    "company",
    "studios",
    "studio",
    "games",
    "game",
    "entertainment",
    "interactive",
    "media",
    "group",
    "tech",
    "technologies",
    "technology",
    "systems",
    "system",
    "support",
    "helper",
    "companion",
    "updater",
    "launcher",
    "client",
    "desktop",
    "version",
    "macos",
    "windows",
];

/// Words from a folder/app name, skipping generic legal/product filler.
pub fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .map(slug)
        .filter(|t| {
            t.len() >= 4
                && !t.chars().all(|c| c.is_ascii_digit())
                && !GENERIC_TOKENS.contains(&t.as_str())
        })
        .collect()
}

pub fn tokens_match(leftover: &str, installed: &str) -> bool {
    let a = tokens(leftover);
    let b = tokens(installed);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.iter().any(|x| b.iter().any(|y| token_pair_match(x, y)))
}

fn token_pair_match(a: &str, b: &str) -> bool {
    if a == b {
        return a.len() >= 4;
    }
    let n = common_prefix_len(a, b);
    n >= 5 && n * 10 >= a.len().min(b.len()) * 8
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

pub fn is_protected_name(name: &str) -> bool {
    let s = slug(name);
    PROTECTED.iter().any(|p| s == slug(p)) || is_macos_system_data(name)
}

/// Apple data that does not use a `com.apple.*` folder name.
pub fn is_macos_system_data(name: &str) -> bool {
    let s = slug(name);
    MACOS_SYSTEM_DATA.iter().any(|p| s == *p) || s.starts_with("comapple")
}

const PROTECTED: &[&str] = &[
    // unix / xdg
    "ssh",
    "gnupg",
    "gpg",
    "git",
    "systemd",
    "dbus",
    "fontconfig",
    "dconf",
    "pulse",
    "pipewire",
    "xdg",
    "gtk-2.0",
    "gtk-3.0",
    "gtk-4.0",
    "glib-2.0",
    "ibus",
    "mimeapps.list",
    "user-dirs.dirs",
    "ku",
    // macOS system-ish
    "addressbook",
    "icloud",
    "clouddocs",
    "mobilesync",
    "callhistorydb",
    "callhistorytransactions",
    "knowledge",
    "suggestions",
    "screenshots",
    "homeenergy",
    "com.apple.sharedfilelist",
    "com.apple.TCC",
    "groupcontainers",
    "application support",
    "caches",
    "preferences",
    "containers",
    "logs",
];

const MACOS_SYSTEM_DATA: &[&str] = &[
    "dictationmodels",
    "geoservices",
    "callhistorydb",
    "callhistorytransactions",
    "knowledge",
    "mobilesync",
    "syncservices",
    "facetime",
    "crashreporter",
    "icloud",
    "clouddocs",
    "addressbook",
    "corespotlight",
    "siri",
    "voiceservices",
    "speechrecognition",
    "speechsynthesis",
    "tts",
    "assistant",
    "donotdisturb",
    "differentialprivacy",
    "fileprovider",
    "cloudkit",
    "ubiquity",
    "identityservices",
    "imessage",
    "messages",
    "mail",
    "notes",
    "reminders",
    "calendar",
    "contacts",
    "safari",
    "preview",
    "quicktime",
];

/// Top-level /var/lib entries that are the OS, not leftover apps.
pub fn is_system_var_lib(name: &str) -> bool {
    let s = slug(name);
    SYSTEM_VAR_LIB.iter().any(|p| s == *p)
}

const SYSTEM_VAR_LIB: &[&str] = &[
    "apt",
    "dpkg",
    "rpm",
    "yum",
    "dnf",
    "pacman",
    "portage",
    "systemd",
    "dbus",
    "sudo",
    "pam",
    "polkit",
    "udev",
    "nfs",
    "modules",
    "dkms",
    "grub",
    "shim",
    "fwupd",
    "packagekit",
    "udisks2",
    "NetworkManager",
    "bluetooth",
    "libvirt",
    "containers",
    "web-ui",
    "misc",
    "private",
    "locate",
    "mlocate",
    "man-db",
    "xml-core",
    "ucf",
    "emacsen-common",
    "ispell",
    "python",
    "python3",
    "plymouth",
    "color",
    "swcatalog",
    "apparmor",
    "snapd",
];

pub fn is_system_library_support(name: &str) -> bool {
    let s = slug(name);
    matches!(
        s.as_str(),
        "apple"
            | "appleinc"
            | "garageband"
            | "logic"
            | "proapps"
            | "coreanalytics"
            | "crashreporter"
            | "google" // vendor root handled separately
    ) || s.starts_with("comapple")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_ids() {
        assert!(looks_like_bundle_id("com.tinyspeck.slackmacgap"));
        assert_eq!(
            extract_bundle_id("ABCDEFG123.com.foo.bar"),
            Some("com.foo.bar".into())
        );
        assert_eq!(
            extract_bundle_id("com.foo.bar.plist"),
            Some("com.foo.bar".into())
        );
        assert!(extract_bundle_id("Slack").is_none());
        assert!(is_apple_id("com.apple.Safari"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Visual Studio Code"), "visualstudiocode");
        assert_eq!(slug("gtk-3.0"), "gtk30");
    }

    #[test]
    fn propellerhead_tokens_match_reason() {
        assert!(tokens_match(
            "Propellerhead Software",
            "se.propellerheads.reason"
        ));
        assert!(tokens_match(
            "Propellerhead Software",
            "Propellerhead Software AB"
        ));
        assert!(!tokens_match("Snacktorio", "Reason"));
    }

    #[test]
    fn dictation_is_macos_system() {
        assert!(is_macos_system_data("DictationModels"));
        assert!(is_protected_name("DictationModels"));
    }
}
