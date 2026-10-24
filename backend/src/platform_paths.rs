// TODO(ai-review): review for style and correctness
//! Unity writes each platform's Addressables build under a folder named for its build target
//! (`StreamingAssets/aa/StandaloneLinux64/…`), so the same asset of a Linux and a Windows depot
//! sits at different paths.

use std::borrow::Cow;

use steam_vent_depot::{DepotFile, Manifest};

const BUILD_TARGETS: &[&str] = &[
    "StandaloneWindows64",
    "StandaloneWindows",
    "StandaloneLinux64",
    "StandaloneOSX",
];
const PLACEHOLDER: &str = "{build-target}";

/// `path` with its Addressables build-target folder replaced by a placeholder, so corresponding
/// files of different platform depots compare equal.
pub fn platform_neutral(path: &str) -> Cow<'_, str> {
    let Some(aa) = path.find("/aa/") else {
        return Cow::Borrowed(path);
    };
    let (head, rest) = path.split_at(aa + "/aa/".len());
    for target in BUILD_TARGETS {
        if let Some(tail) = rest.strip_prefix(target)
            && tail.starts_with('/')
        {
            return Cow::Owned(format!("{head}{PLACEHOLDER}{tail}"));
        }
    }
    Cow::Borrowed(path)
}

/// The file at `path`, else the one at the corresponding path under another build target.
pub fn find_corresponding<'m>(manifest: &'m Manifest, path: &str) -> Option<&'m DepotFile> {
    if let Some(file) = manifest.files.iter().find(|f| f.path == path) {
        return Some(file);
    }
    let neutral = platform_neutral(path);
    if matches!(neutral, Cow::Borrowed(_)) {
        return None;
    }
    manifest
        .files
        .iter()
        .find(|f| platform_neutral(&f.path) == neutral)
}

/// The path of [`find_corresponding`]'s file, or `path` itself when there is none.
pub fn resolve_path(manifest: &Manifest, path: &str) -> String {
    find_corresponding(manifest, path).map_or_else(|| path.to_owned(), |f| f.path.clone())
}

#[cfg(test)]
mod tests {
    use super::platform_neutral;

    #[test]
    fn replaces_only_the_addressables_build_target_folder() {
        assert_eq!(
            platform_neutral("Game_Data/StreamingAssets/aa/StandaloneLinux64/a.bundle"),
            "Game_Data/StreamingAssets/aa/{build-target}/a.bundle"
        );
        assert_eq!(
            platform_neutral("Game_Data/StreamingAssets/aa/StandaloneWindows64/a.bundle"),
            "Game_Data/StreamingAssets/aa/{build-target}/a.bundle"
        );
        assert_eq!(
            platform_neutral("Game_Data/StreamingAssets/aa/AddressablesLink/link.xml"),
            "Game_Data/StreamingAssets/aa/AddressablesLink/link.xml"
        );
        assert_eq!(
            platform_neutral("Game_Data/StreamingAssets/aa/StandaloneWindows64Extra/x"),
            "Game_Data/StreamingAssets/aa/StandaloneWindows64Extra/x"
        );
    }
}
