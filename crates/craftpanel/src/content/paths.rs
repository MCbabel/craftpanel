use std::path::{Path, PathBuf};

pub const MAX_SEGMENT: usize = 255;
pub const MAX_PATH: usize = 4096;
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFault {
    Invalid,
    TooLong,
    Escapes,
}

impl PathFault {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_path",
            Self::TooLong => "path_too_long",
            Self::Escapes => "forbidden_path",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Invalid => "that path cannot be used",
            Self::TooLong => "that path is too long",
            Self::Escapes => "that path leads out of the server directory",
        }
    }
}

pub type Result<T> = std::result::Result<T, PathFault>;

pub fn normalise(raw: &str) -> Result<Vec<String>> {
    if raw.len() > MAX_PATH {
        return Err(PathFault::TooLong);
    }
    if raw.contains('\0') {
        return Err(PathFault::Invalid);
    }

    let mut segments = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return Err(PathFault::Invalid),
            _ => {}
        }
        if segment.len() > MAX_SEGMENT {
            return Err(PathFault::TooLong);
        }
        segments.push(segment.to_owned());
    }

    if segments.len() > MAX_DEPTH {
        return Err(PathFault::TooLong);
    }
    Ok(segments)
}

pub fn relative(raw: &str) -> Result<String> {
    Ok(normalise(raw)?.join("/"))
}

pub fn leading_slash(relative: &str) -> String {
    format!("/{}", relative.trim_start_matches('/'))
}

pub fn resolve(root: &Path, raw: &str) -> Result<PathBuf> {
    let segments = normalise(raw)?;
    let root = root.canonicalize().map_err(|_| PathFault::Escapes)?;
    walk(&root, &segments)
}

pub fn resolve_leaf(root: &Path, raw: &str) -> Result<PathBuf> {
    let mut segments = normalise(raw)?;
    let last = segments.pop().ok_or(PathFault::Invalid)?;
    let root = root.canonicalize().map_err(|_| PathFault::Escapes)?;
    Ok(walk(&root, &segments)?.join(last))
}

fn walk(root: &Path, segments: &[String]) -> Result<PathBuf> {
    let mut here = root.to_path_buf();
    for segment in segments {
        here.push(segment);
        let Ok(meta) = std::fs::symlink_metadata(&here) else { continue };
        if !meta.file_type().is_symlink() {
            continue;
        }
        let target = here.canonicalize().map_err(|_| PathFault::Escapes)?;
        if !target.starts_with(root) {
            return Err(PathFault::Escapes);
        }
        here = target;
    }
    Ok(here)
}

pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn clear_destination(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => remove(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("craftpanel-paths-{name}-{}", crate::model::Id::new()));
            std::fs::create_dir_all(path.join("mods")).expect("a server directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_leading_slash_means_nothing_and_dot_segments_fall_away() {
        assert_eq!(normalise("/mods/foo.jar").unwrap(), ["mods", "foo.jar"]);
        assert_eq!(normalise("mods//./foo.jar").unwrap(), ["mods", "foo.jar"]);
        assert_eq!(normalise("").unwrap(), Vec::<String>::new());
        assert_eq!(normalise("/").unwrap(), Vec::<String>::new());
        assert_eq!(normalise(r"mods\evil.jar").unwrap(), [r"mods\evil.jar"]);
    }

    #[test]
    fn a_dot_dot_segment_is_refused_and_never_folded_in() {
        for raw in ["../etc/passwd", "mods/../../etc", "mods/../mods/foo.jar", ".."] {
            assert_eq!(normalise(raw), Err(PathFault::Invalid), "{raw}");
        }
    }

    #[test]
    fn the_three_size_limits_of_n6_are_separate_answers() {
        assert_eq!(normalise(&"a".repeat(MAX_SEGMENT + 1)), Err(PathFault::TooLong));
        assert_eq!(normalise(&"a/".repeat(MAX_DEPTH + 1)), Err(PathFault::TooLong));
        assert_eq!(normalise(&"a".repeat(MAX_PATH + 1)), Err(PathFault::TooLong));
        assert_eq!(normalise("mods/\0.jar"), Err(PathFault::Invalid));
    }

    #[test]
    fn a_symlink_out_of_the_server_directory_is_refused() {
        let root = Root::new("escape");
        let outside = root.path().parent().expect("a parent").join("outside.txt");
        std::fs::write(&outside, b"panel.db").expect("a file outside");
        std::os::unix::fs::symlink(&outside, root.path().join("mods").join("link.jar"))
            .expect("a link");

        assert_eq!(resolve(root.path(), "mods/link.jar"), Err(PathFault::Escapes));
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn a_symlinked_directory_that_points_outside_is_refused_before_the_file_is_named() {
        let root = Root::new("dir-escape");
        let elsewhere = root.path().parent().expect("a parent").join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("a directory outside");
        std::fs::write(elsewhere.join("foo.jar"), b"x").expect("a file");
        std::os::unix::fs::symlink(&elsewhere, root.path().join("plugins")).expect("a link");

        assert_eq!(resolve(root.path(), "plugins/foo.jar"), Err(PathFault::Escapes));
        assert_eq!(resolve_leaf(root.path(), "plugins/new.jar"), Err(PathFault::Escapes));
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn a_link_that_stays_inside_is_followed_and_kept() {
        let root = Root::new("inside");
        std::fs::create_dir_all(root.path().join("real")).expect("a directory");
        std::fs::write(root.path().join("real").join("foo.jar"), b"x").expect("a file");
        std::os::unix::fs::symlink(root.path().join("real"), root.path().join("mods").join("here"))
            .expect("a link");

        let resolved = resolve(root.path(), "mods/here/foo.jar").expect("it stays inside");
        assert_eq!(resolved, root.path().canonicalize().unwrap().join("real").join("foo.jar"));
    }

    #[test]
    fn a_path_that_does_not_exist_yet_resolves_to_where_it_would_go() {
        let root = Root::new("fresh");
        let resolved = resolve_leaf(root.path(), "mods/new.jar").expect("a place to write");
        assert_eq!(resolved, root.path().canonicalize().unwrap().join("mods").join("new.jar"));
        assert!(!resolved.exists());
    }

    #[test]
    fn writing_over_a_link_unlinks_it_and_leaves_the_target_alone() {
        let root = Root::new("clear");
        let outside = root.path().parent().expect("a parent").join("target.txt");
        std::fs::write(&outside, b"keep me").expect("a file outside");
        let link = root.path().join("mods").join("foo.jar");
        std::os::unix::fs::symlink(&outside, &link).expect("a link");

        clear_destination(&link).expect("the link goes");
        assert!(!link.exists());
        assert_eq!(std::fs::read(&outside).expect("the target stays"), b"keep me");
        let _ = std::fs::remove_file(&outside);
    }
}
