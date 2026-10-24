// TODO(ai-review): review for style and correctness
//! Unity writes each platform's Addressables build under a folder named for its build target
//! (`StreamingAssets/aa/StandaloneLinux64/…`), so the same asset of a Linux and a Windows depot
//! sits at different paths.

use std::collections::HashMap;

use steam_vent_depot::{DepotFile, Manifest};

const BUILD_TARGETS: &[&str] = &[
    "StandaloneWindows64",
    "StandaloneWindows",
    "StandaloneLinux64",
    "StandaloneOSX",
];

/// A path's identity across platform depots: the parts before and after its Addressables
/// build-target folder, or the whole path and `""` when it has none.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PairKey<'a> {
    head: &'a str,
    tail: &'a str,
}

impl<'a> PairKey<'a> {
    pub fn new(path: &'a str) -> Self {
        let whole = PairKey {
            head: path,
            tail: "",
        };
        let Some(aa) = path.find("/aa/") else {
            return whole;
        };
        let (head, rest) = path.split_at(aa + "/aa/".len());
        for target in BUILD_TARGETS {
            if let Some(tail) = rest.strip_prefix(target)
                && tail.starts_with('/')
            {
                return PairKey { head, tail };
            }
        }
        whole
    }

    fn has_build_target(self) -> bool {
        !self.tail.is_empty()
    }

    pub fn to_owned(self) -> (String, String) {
        (self.head.to_owned(), self.tail.to_owned())
    }
}

/// The file at `path`, else the one at the corresponding path under another build target.
pub fn find_corresponding<'m>(manifest: &'m Manifest, path: &str) -> Option<&'m DepotFile> {
    if let Some(file) = manifest.files.iter().find(|f| f.path == path) {
        return Some(file);
    }
    let key = PairKey::new(path);
    if !key.has_build_target() {
        return None;
    }
    manifest.files.iter().find(|f| PairKey::new(&f.path) == key)
}

/// The path of [`find_corresponding`]'s file, or `path` itself when there is none.
pub fn resolve_path(manifest: &Manifest, path: &str) -> String {
    find_corresponding(manifest, path).map_or_else(|| path.to_owned(), |f| f.path.clone())
}

/// A manifest's non-directory files in manifest order, keyed by [`PairKey`].
pub struct FileIndex<'m> {
    files: Vec<(PairKey<'m>, &'m DepotFile)>,
    by_key: HashMap<PairKey<'m>, &'m DepotFile>,
}

/// One file identity across two [`FileIndex`]es.
#[derive(Clone, Copy)]
pub enum Paired<'b, 'o> {
    Both(&'b DepotFile, &'o DepotFile),
    BaseOnly(&'b DepotFile),
    OtherOnly(&'o DepotFile),
}

impl<'m> FileIndex<'m> {
    pub fn new(manifest: &'m Manifest) -> Self {
        let files: Vec<_> = manifest
            .files
            .iter()
            .filter(|f| !f.is_dir())
            .map(|f| (PairKey::new(&f.path), f))
            .collect();
        let by_key = files.iter().copied().collect();
        FileIndex { files, by_key }
    }

    pub fn files(&self) -> impl Iterator<Item = &'m DepotFile> + '_ {
        self.files.iter().map(|(_, f)| *f)
    }

    /// Every file identity of `self` and `other`; `self`'s files first, each side in manifest
    /// order.
    pub fn pair<'o>(
        &'o self,
        other: &'o FileIndex<'o>,
    ) -> impl Iterator<Item = (PairKey<'o>, Paired<'m, 'o>)> {
        let ours = self.files.iter().map(|&(key, f)| {
            let paired = match other.by_key.get(&key) {
                Some(o) => Paired::Both(f, o),
                None => Paired::BaseOnly(f),
            };
            (key, paired)
        });
        let theirs = other
            .files
            .iter()
            .filter(|(key, _)| !self.by_key.contains_key(key))
            .map(|&(key, f)| (key, Paired::OtherOnly(f)));
        ours.chain(theirs)
    }
}

#[cfg(test)]
mod tests {
    use super::PairKey;

    #[test]
    fn pairs_only_the_addressables_build_target_folder() {
        let key = |p| PairKey::new(p);
        assert_eq!(
            key("Game_Data/StreamingAssets/aa/StandaloneLinux64/a.bundle"),
            key("Game_Data/StreamingAssets/aa/StandaloneWindows64/a.bundle"),
        );
        assert_eq!(
            key("Game_Data/StreamingAssets/aa/StandaloneLinux64/a.bundle"),
            PairKey {
                head: "Game_Data/StreamingAssets/aa/",
                tail: "/a.bundle"
            }
        );
        assert_eq!(
            key("Game_Data/StreamingAssets/aa/AddressablesLink/link.xml"),
            PairKey {
                head: "Game_Data/StreamingAssets/aa/AddressablesLink/link.xml",
                tail: ""
            }
        );
        assert_eq!(
            key("Game_Data/StreamingAssets/aa/StandaloneWindows64Extra/x"),
            PairKey {
                head: "Game_Data/StreamingAssets/aa/StandaloneWindows64Extra/x",
                tail: ""
            }
        );
    }
}
