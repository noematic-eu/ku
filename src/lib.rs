pub mod app;
pub mod collector;
pub mod config;
pub mod hits;
pub mod orphans;
pub mod paths;
pub mod storage;
pub mod theme;
pub mod ui;
pub mod utils;

/// CLI / README / help banner. Two-letter mark, terminal-native.
pub const BANNER: &str = "\
 _           
| | ___ _   _ 
| |/ / | | | |
|   <| |_| |
|_|\\_\\ \\__,_|";

pub const BANNER_TAGLINE: &str = "htop + df + ncdu";

#[cfg(test)]
mod banner_tests {
    use super::*;

    #[test]
    fn banner_is_five_lines_and_looks_like_ku() {
        let lines: Vec<&str> = BANNER.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains('_'));
        assert!(lines[4].contains("\\__,_|"));
    }
}
