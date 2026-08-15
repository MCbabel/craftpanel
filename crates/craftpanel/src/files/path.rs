use std::fmt;

pub const MAX_SEGMENT_BYTES: usize = 255;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFault {
    Invalid,
    TooLong,
    Name,
}

impl PathFault {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_path",
            Self::TooLong => "path_too_long",
            Self::Name => "invalid_name",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::Invalid => "this path cannot be used: no '..', no null bytes",
            Self::TooLong => "this path is too long",
            Self::Name => "this name cannot be used",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelPath {
    segments: Vec<String>,
}

impl RelPath {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn parse(raw: &str) -> Result<Self, PathFault> {
        if raw.contains('\0') {
            return Err(PathFault::Invalid);
        }
        if raw.len() > MAX_PATH_BYTES {
            return Err(PathFault::TooLong);
        }

        let mut segments = Vec::new();
        for piece in raw.split('/') {
            if piece.is_empty() || piece == "." {
                continue;
            }
            if piece == ".." {
                return Err(PathFault::Invalid);
            }
            if piece.len() > MAX_SEGMENT_BYTES {
                return Err(PathFault::TooLong);
            }
            segments.push(piece.to_owned());
        }

        if segments.len() > MAX_DEPTH {
            return Err(PathFault::TooLong);
        }
        let joined = segments.iter().map(|s| s.len() + 1).sum::<usize>();
        if joined > MAX_PATH_BYTES {
            return Err(PathFault::TooLong);
        }

        Ok(Self { segments })
    }

    pub fn parse_bytes(raw: &[u8]) -> Result<Self, PathFault> {
        std::str::from_utf8(raw).map_err(|_| PathFault::Invalid).and_then(Self::parse)
    }

    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn depth(&self) -> usize {
        self.segments.len()
    }

    pub fn name(&self) -> Option<&str> {
        self.segments.last().map(String::as_str)
    }

    pub fn parent(&self) -> Self {
        let mut segments = self.segments.clone();
        segments.pop();
        Self { segments }
    }

    pub fn join(&self, other: &Self) -> Result<Self, PathFault> {
        let mut segments = self.segments.clone();
        segments.extend(other.segments.iter().cloned());
        if segments.len() > MAX_DEPTH {
            return Err(PathFault::TooLong);
        }
        if segments.iter().map(|s| s.len() + 1).sum::<usize>() > MAX_PATH_BYTES {
            return Err(PathFault::TooLong);
        }
        Ok(Self { segments })
    }

    pub fn child(&self, name: &str) -> Result<Self, PathFault> {
        check_name(name)?;
        self.join(&Self { segments: vec![name.to_owned()] })
    }

    pub fn with_name(&self, name: &str) -> Self {
        let mut segments = self.segments.clone();
        segments.push(name.to_owned());
        Self { segments }
    }

    pub fn starts_with(&self, other: &Self) -> bool {
        other.segments.len() <= self.segments.len()
            && other.segments.iter().zip(&self.segments).all(|(a, b)| a == b)
    }

    pub fn on_the_wire(&self) -> String {
        format!("/{}", self.segments.join("/"))
    }

    pub fn beneath_root(&self) -> String {
        if self.segments.is_empty() {
            ".".to_owned()
        } else {
            self.segments.join("/")
        }
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.on_the_wire())
    }
}

pub fn check_name(name: &str) -> Result<(), PathFault> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(PathFault::Name);
    }
    if name.contains('/') || name.contains('\0') || name.chars().any(char::is_control) {
        return Err(PathFault::Name);
    }
    if name.len() > MAX_SEGMENT_BYTES {
        return Err(PathFault::TooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(raw: &str) -> Vec<String> {
        RelPath::parse(raw).expect("a usable path").segments().to_vec()
    }

    #[test]
    fn both_shapes_the_layout_produces_mean_the_same_thing() {
        assert_eq!(segments("plugins/config"), ["plugins", "config"]);
        assert_eq!(segments("/plugins"), ["plugins"]);
        assert_eq!(segments("/plugins/"), ["plugins"]);
        assert_eq!(segments("./plugins//config/."), ["plugins", "config"]);

        for root in ["", "/", ".", "//", "/./"] {
            assert!(RelPath::parse(root).expect("the root").is_root(), "{root:?}");
        }
        assert_eq!(RelPath::root().on_the_wire(), "/");
        assert_eq!(RelPath::parse("a/b").unwrap().on_the_wire(), "/a/b");
    }

    #[test]
    fn a_climbing_segment_is_refused_and_never_folded_away() {
        for climb in ["..", "../etc", "a/../b", "plugins/../../panel.db", "a/b/.."] {
            assert_eq!(
                RelPath::parse(climb),
                Err(PathFault::Invalid),
                "{climb:?} has to be refused, not resolved: a/../b is not b when a is a link"
            );
        }
    }

    #[test]
    fn an_absolute_path_names_the_root_and_nothing_above_it() {
        assert_eq!(segments("/etc/passwd"), ["etc", "passwd"]);
        assert_eq!(segments("//var/lib/craftpanel/panel.db"), ["var", "lib", "craftpanel", "panel.db"]);
    }

    #[test]
    fn a_null_byte_is_refused_wherever_it_sits() {
        assert_eq!(RelPath::parse("a\0b"), Err(PathFault::Invalid));
        assert_eq!(RelPath::parse("plugins/a\0"), Err(PathFault::Invalid));
        assert_eq!(check_name("a\0b"), Err(PathFault::Name));
    }

    #[test]
    fn a_name_that_is_not_utf8_never_becomes_a_path() {
        assert_eq!(RelPath::parse_bytes(b"plugins/ok"), Ok(RelPath::parse("plugins/ok").unwrap()));
        assert_eq!(RelPath::parse_bytes(&[0xff, 0xfe]), Err(PathFault::Invalid));
    }

    #[test]
    fn the_three_ceilings_of_n6() {
        let long = "x".repeat(MAX_SEGMENT_BYTES);
        assert!(RelPath::parse(&long).is_ok());
        assert_eq!(RelPath::parse(&format!("{long}x")), Err(PathFault::TooLong));

        let deep = "a/".repeat(MAX_DEPTH);
        assert_eq!(RelPath::parse(&deep).expect("64 deep").depth(), MAX_DEPTH);
        assert_eq!(RelPath::parse(&format!("{deep}a")), Err(PathFault::TooLong));

        let wide = std::iter::repeat_n("abcdefgh", 600).collect::<Vec<_>>().join("/");
        assert_eq!(RelPath::parse(&wide), Err(PathFault::TooLong));
    }

    #[test]
    fn a_name_is_refused_for_what_the_server_refuses_and_nothing_more() {
        for bad in ["", ".", "..", "a/b", "a\nb", "\u{7}"] {
            assert_eq!(check_name(bad), Err(PathFault::Name), "{bad:?}");
        }
        for good in ["server.properties", "Renée (2).txt", "a b", ".hidden", "-"] {
            assert_eq!(check_name(good), Ok(()), "{good:?}");
        }
        assert_eq!(check_name(&"x".repeat(MAX_SEGMENT_BYTES + 1)), Err(PathFault::TooLong));
    }

    #[test]
    fn a_move_into_ones_own_subtree_is_recognisable() {
        let source = RelPath::parse("/world").unwrap();
        let inside = RelPath::parse("/world/region/backup").unwrap();
        let beside = RelPath::parse("/world-old").unwrap();

        assert!(inside.starts_with(&source));
        assert!(!beside.starts_with(&source), "a name that merely begins the same is not inside");
        assert!(source.starts_with(&source));
        assert!(inside.starts_with(&RelPath::root()), "everything is inside the root");
    }

    #[test]
    fn what_the_kernel_is_handed_is_never_absolute() {
        assert_eq!(RelPath::root().beneath_root(), ".");
        assert_eq!(RelPath::parse("/a/b").unwrap().beneath_root(), "a/b");
        assert!(!RelPath::parse("/a").unwrap().beneath_root().starts_with('/'));
    }
}
