#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Properties {
    lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Verbatim(String),
    Entry {
        key: String,
        value: String,
        original: Option<String>,
    },
}

impl Properties {
    pub fn parse(text: &str) -> Self {
        let physical = split_physical(text);
        let mut lines = Vec::new();
        let mut at = 0;

        while at < physical.len() {
            let line = physical[at];
            if line.trim().is_empty() || starts_a_comment(line) {
                lines.push(Line::Verbatim(line.to_owned()));
                at += 1;
                continue;
            }

            let mut end = at;
            while end < physical.len() - 1 && continues(physical[end]) {
                end += 1;
            }
            let part = &physical[at..=end];
            let (key, value) = split_entry(&join_logical(part));
            lines.push(Line::Entry { key, value, original: Some(part.join("\n")) });
            at = end + 1;
        }

        Self { lines }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().rev().find_map(|line| match line {
            Line::Entry { key: found, value, .. } if found == key => Some(value.as_str()),
            _ => None,
        })
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.lines.iter().filter_map(|line| match line {
            Line::Entry { key, value, .. } => Some((key.as_str(), value.as_str())),
            Line::Verbatim(_) => None,
        })
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let existing = self.lines.iter_mut().rev().find(|line| match line {
            Line::Entry { key: found, .. } => found == key,
            Line::Verbatim(_) => false,
        });

        match existing {
            Some(Line::Entry { value: held, original, .. }) => {
                if held != value {
                    *held = value.to_owned();
                    *original = None;
                }
            }
            _ => self.lines.push(Line::Entry {
                key: key.to_owned(),
                value: value.to_owned(),
                original: None,
            }),
        }
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.lines.len();
        self.lines.retain(|line| !matches!(line, Line::Entry { key: found, .. } if found == key));
        self.lines.len() != before
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                Line::Verbatim(text) => out.push_str(text),
                Line::Entry { original: Some(text), .. } => out.push_str(text),
                Line::Entry { key, value, original: None } => {
                    out.push_str(&escape(key, true));
                    out.push('=');
                    out.push_str(&escape(value, false));
                }
            }
            out.push('\n');
        }
        out
    }
}

fn split_physical(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> =
        text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    if lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    lines
}

fn starts_a_comment(line: &str) -> bool {
    matches!(line.trim_start().as_bytes().first(), Some(b'#' | b'!'))
}

fn continues(line: &str) -> bool {
    line.bytes().rev().take_while(|byte| *byte == b'\\').count() % 2 == 1
}

fn join_logical(parts: &[&str]) -> String {
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        let piece = if index == 0 { *part } else { part.trim_start() };
        if index + 1 < parts.len() {
            out.push_str(&piece[..piece.len() - 1]);
        } else {
            out.push_str(piece);
        }
    }
    out
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | 0x0c)
}

fn split_entry(logical: &str) -> (String, String) {
    let bytes = logical.as_bytes();
    let mut at = 0;
    while at < bytes.len() && is_space(bytes[at]) {
        at += 1;
    }

    let key_start = at;
    let mut escaped = false;
    while at < bytes.len() {
        let byte = bytes[at];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if is_space(byte) || byte == b'=' || byte == b':' {
            break;
        }
        at += 1;
    }
    let key = &logical[key_start..at];

    while at < bytes.len() && is_space(bytes[at]) {
        at += 1;
    }
    if at < bytes.len() && (bytes[at] == b'=' || bytes[at] == b':') {
        at += 1;
        while at < bytes.len() && is_space(bytes[at]) {
            at += 1;
        }
    }

    (unescape(key), unescape(&logical[at..]))
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();

    while let Some(letter) = chars.next() {
        if letter != '\\' {
            out.push(letter);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let digits: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    None => {
                        out.push_str("\\u");
                        out.push_str(&digits);
                    }
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn escape(text: &str, is_key: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, letter) in text.chars().enumerate() {
        match letter {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{c}' => out.push_str("\\f"),
            '=' | ':' | '#' | '!' => {
                out.push('\\');
                out.push(letter);
            }
            ' ' if is_key || index == 0 => out.push_str("\\ "),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_FILE: &str = "\
#Minecraft server properties
#Tue Aug 12 09:14:02 UTC 2026
enable-jmx-monitoring=false
rcon.port=25575
level-seed=
gamemode=survival
enable-command-block=false
motd=A Minecraft Server
query.port=25565
spawn-protection=16
white-list=false
";

    const WHOLE_FILE: &str = "\
#Minecraft server properties
#Thu Aug 07 12:00:00 UTC 2026
accepts-transfers=false
allow-flight=false
allow-nether=true
broadcast-console-to-ops=true
broadcast-rcon-to-ops=true
bug-report-link=
difficulty=easy
enable-command-block=false
enable-jmx-monitoring=false
enable-query=false
enable-rcon=false
enable-status=true
enforce-secure-profile=true
enforce-whitelist=false
entity-broadcast-range-percentage=100
force-gamemode=false
function-permission-level=2
gamemode=survival
generate-structures=true
generator-settings={}
hardcore=false
hide-online-players=false
initial-disabled-packs=
initial-enabled-packs=vanilla
level-name=world
level-seed=
level-type=minecraft\\:normal
log-ips=true
max-chained-neighbor-updates=1000000
max-players=20
max-tick-time=60000
max-world-size=29999984
motd=A Minecraft Server
network-compression-threshold=256
online-mode=true
op-permission-level=4
pause-when-empty-seconds=60
player-idle-timeout=0
prevent-proxy-connections=false
pvp=true
query.port=25565
rate-limit=0
rcon.password=
rcon.port=25575
region-file-compression=deflate
require-resource-pack=false
resource-pack=
resource-pack-id=
resource-pack-prompt=
resource-pack-sha1=
server-ip=
server-port=25565
simulation-distance=10
spawn-monsters=true
spawn-protection=16
sync-chunk-writes=true
text-filtering-config=
use-native-transport=true
view-distance=10
white-list=false
";

    #[test]
    fn the_whole_file_of_a_real_server_reads_and_writes_back_byte_for_byte() {
        let parsed = Properties::parse(WHOLE_FILE);

        assert_eq!(parsed.entries().count(), 60);
        assert_eq!(parsed.get("level-type"), Some("minecraft:normal"), "Java escapes the colon");
        assert_eq!(parsed.get("generator-settings"), Some("{}"));
        assert_eq!(parsed.get("bug-report-link"), Some(""));
        assert_eq!(parsed.get("motd"), Some("A Minecraft Server"));
        assert_eq!(parsed.render(), WHOLE_FILE, "an untouched file must not move");

        let mut edited = Properties::parse(WHOLE_FILE);
        edited.set("motd", "At home with Anna");
        let written = edited.render();
        assert_eq!(
            written.lines().filter(|line| *line != "motd=At home with Anna").count(),
            WHOLE_FILE.lines().filter(|line| *line != "motd=A Minecraft Server").count()
        );
        assert!(written.contains("level-type=minecraft\\:normal\n"), "{written}");
        assert_eq!(Properties::parse(&written).get("level-type"), Some("minecraft:normal"));
    }

    #[test]
    fn a_real_file_survives_a_read_and_a_write_unchanged() {
        let parsed = Properties::parse(REAL_FILE);
        assert_eq!(parsed.render(), REAL_FILE, "an untouched file must not move");

        assert_eq!(parsed.get("motd"), Some("A Minecraft Server"));
        assert_eq!(parsed.get("level-seed"), Some(""));
        assert_eq!(parsed.get("query.port"), Some("25565"));
        assert_eq!(parsed.get("nothing-of-the-sort"), None);
    }

    #[test]
    fn writing_one_key_leaves_the_comments_and_the_order_alone() {
        let mut parsed = Properties::parse(REAL_FILE);
        parsed.set("motd", "At home with Anna");
        parsed.set("difficulty", "hard");
        assert!(parsed.remove("white-list"));

        let written = parsed.render();
        assert!(written.starts_with("#Minecraft server properties\n#Tue Aug 12"));
        assert!(written.contains("motd=At home with Anna\n"));
        assert!(!written.contains("white-list"));
        assert!(written.trim_end().ends_with("difficulty=hard"), "a new key goes last: {written}");

        let again = Properties::parse(&written);
        let keys: Vec<&str> = again.entries().map(|(key, _)| key).collect();
        assert_eq!(
            keys,
            [
                "enable-jmx-monitoring",
                "rcon.port",
                "level-seed",
                "gamemode",
                "enable-command-block",
                "motd",
                "query.port",
                "spawn-protection",
                "difficulty",
            ]
        );
    }

    #[test]
    fn the_three_separators_and_the_bare_key_all_read() {
        let parsed = Properties::parse("a=1\nb:2\nc 3\n  d   =   4\ne\nf=\n");

        assert_eq!(parsed.get("a"), Some("1"));
        assert_eq!(parsed.get("b"), Some("2"));
        assert_eq!(parsed.get("c"), Some("3"));
        assert_eq!(parsed.get("d"), Some("4"));
        assert_eq!(parsed.get("e"), Some(""));
        assert_eq!(parsed.get("f"), Some(""));
    }

    #[test]
    fn a_trailing_backslash_carries_the_value_onto_the_next_line() {
        let parsed = Properties::parse("motd=one \\\n    two \\\n    three\nnext=4\n");

        assert_eq!(parsed.get("motd"), Some("one two three"));
        assert_eq!(parsed.get("next"), Some("4"));
        assert_eq!(parsed.render(), "motd=one \\\n    two \\\n    three\nnext=4\n");
    }

    #[test]
    fn an_even_number_of_backslashes_is_a_value_and_not_a_continuation() {
        let parsed = Properties::parse("windows=C\\:\\\\srv\\\\\nnext=4\n");

        assert_eq!(parsed.get("windows"), Some("C:\\srv\\"));
        assert_eq!(parsed.get("next"), Some("4"), "the next line is its own entry");
        assert_eq!(parsed.entries().count(), 2);
    }

    #[test]
    fn the_escapes_of_java_properties_read_and_write_back() {
        let parsed = Properties::parse(
            "motd=\\u00a76Gold\\u00a7r and \\tTab\nweird\\=key=yes\nspaced\\ key=no\n",
        );

        assert_eq!(parsed.get("motd"), Some("§6Gold§r and \tTab"));
        assert_eq!(parsed.get("weird=key"), Some("yes"));
        assert_eq!(parsed.get("spaced key"), Some("no"));

        let mut rewritten = Properties::default();
        rewritten.set("motd", "§6Gold§r and \tTab");
        rewritten.set("weird=key", "yes");
        rewritten.set("spaced key", "no");
        rewritten.set("leading", " space");
        rewritten.set("sharp", "a#b");

        assert_eq!(
            rewritten.render(),
            "motd=§6Gold§r and \\tTab\nweird\\=key=yes\nspaced\\ key=no\n\
             leading=\\ space\nsharp=a\\#b\n"
        );
        let again = Properties::parse(&rewritten.render());
        assert_eq!(again.get("motd"), Some("§6Gold§r and \tTab"));
        assert_eq!(again.get("leading"), Some(" space"));
        assert_eq!(again.get("sharp"), Some("a#b"));
        assert_eq!(again.get("weird=key"), Some("yes"));
    }

    #[test]
    fn a_comment_is_a_comment_even_when_it_looks_like_a_key() {
        let parsed = Properties::parse("#motd=hidden\n!also=hidden\n   # indented\nmotd=shown\n");

        assert_eq!(parsed.get("motd"), Some("shown"));
        assert_eq!(parsed.get("also"), None);
        assert_eq!(parsed.entries().count(), 1);
    }

    #[test]
    fn the_last_spelling_wins_as_it_does_in_java() {
        let mut parsed = Properties::parse("motd=first\nmotd=second\n");
        assert_eq!(parsed.get("motd"), Some("second"));

        parsed.set("motd", "third");
        assert_eq!(parsed.render(), "motd=first\nmotd=third\n");

        assert!(parsed.remove("motd"));
        assert_eq!(parsed.render(), "");
    }

    #[test]
    fn an_empty_file_stays_empty_and_a_file_without_a_newline_gains_one() {
        assert_eq!(Properties::parse("").render(), "");
        assert_eq!(Properties::parse("motd=x").render(), "motd=x\n");
        assert_eq!(Properties::parse("motd=x\r\nb=y\r\n").get("motd"), Some("x"));
    }
}
