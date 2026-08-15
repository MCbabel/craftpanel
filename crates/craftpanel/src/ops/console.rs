use std::collections::VecDeque;
use std::sync::Arc;

pub const MAX_LINES: usize = 10_000;
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 8 * 1024;
const TRUNCATION_MARK: &str = " [truncated]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    pub first_seq: u64,
    pub lines: Vec<Arc<str>>,
    pub dropped: u64,
}

#[derive(Debug)]
pub struct Console {
    lines: VecDeque<Arc<str>>,
    bytes: usize,
    next_seq: u64,
    first_seq: u64,
    dropped: u64,
}

impl Console {
    pub fn new(start: u64) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            next_seq: start,
            first_seq: start,
            dropped: 0,
        }
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn push(&mut self, raw: &str) -> Arc<str> {
        let line: Arc<str> = Arc::from(tidy(raw));
        self.bytes += line.len();
        self.lines.push_back(Arc::clone(&line));
        self.next_seq += 1;
        while self.lines.len() > MAX_LINES || (self.bytes > MAX_BYTES && self.lines.len() > 1) {
            if let Some(gone) = self.lines.pop_front() {
                self.bytes -= gone.len();
                self.first_seq += 1;
                self.dropped += 1;
            }
        }
        line
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.bytes = 0;
        self.first_seq = self.next_seq;
    }

    pub fn history(&self) -> History {
        History {
            first_seq: self.first_seq,
            lines: self.lines.iter().cloned().collect(),
            dropped: self.dropped,
        }
    }
}

pub fn tidy(raw: &str) -> String {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(letter) = chars.next() {
        match letter {
            '\u{1b}' => {
                match chars.next() {
                    Some('[') => {
                        for following in chars.by_ref() {
                            if following.is_ascii_alphabetic() {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        for following in chars.by_ref() {
                            if following == '\u{7}' || following == '\u{9c}' {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            '\t' => out.push('\t'),
            '\r' | '\n' => {}
            other if other.is_control() => {}
            other => out.push(other),
        }
    }

    if out.len() > MAX_LINE_BYTES {
        let mut cut = MAX_LINE_BYTES - TRUNCATION_MARK.len();
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str(TRUNCATION_MARK);
    }
    out
}

pub fn tail_of_log(path: &std::path::Path, max_lines: usize, max_bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(end) = file.seek(SeekFrom::End(0)) else {
        return Vec::new();
    };
    let from = end.saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.take(max_bytes).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }

    let text = String::from_utf8_lossy(&bytes);
    let mut lines: VecDeque<&str> = text.lines().collect();
    if from > 0 {
        lines.pop_front();
    }
    while lines.len() > max_lines {
        lines.pop_front();
    }
    lines.into_iter().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_survives_both_the_ring_and_the_clearing() {
        let mut console = Console::new(41);
        console.push("first");
        assert_eq!(console.next_seq(), 42);

        console.clear();
        console.push("after the clear");
        let history = console.history();
        assert_eq!(history.first_seq, 42, "clearing must not rewind the count");
        assert_eq!(history.lines.len(), 1);
        assert_eq!(history.dropped, 0);
    }

    #[test]
    fn the_oldest_lines_fall_out_and_are_counted() {
        let mut console = Console::new(0);
        for index in 0..MAX_LINES + 5 {
            console.push(&format!("line {index}"));
        }
        let history = console.history();
        assert_eq!(history.lines.len(), MAX_LINES);
        assert_eq!(history.dropped, 5);
        assert_eq!(history.first_seq, 5);
        assert_eq!(&*history.lines[0], "line 5");
        assert_eq!(history.first_seq + history.lines.len() as u64, console.next_seq());
    }

    #[test]
    fn four_mebibytes_are_the_second_ceiling() {
        let mut console = Console::new(0);
        let fat = "x".repeat(MAX_LINE_BYTES);
        let pushed = 800;
        for _ in 0..pushed {
            console.push(&fat);
        }
        let history = console.history();
        assert!(
            history.lines.len() < pushed,
            "6 MiB of lines are under the line ceiling, so the byte one has to bite"
        );
        let held: usize = history.lines.iter().map(|line| line.len()).sum();
        assert!(held <= MAX_BYTES + fat.len(), "held {held} bytes");
        assert_eq!(history.dropped, pushed as u64 - history.lines.len() as u64);
    }

    #[test]
    fn the_minecraft_timestamp_stays_at_the_very_front() {
        let coloured = "\u{feff}\u{1b}[32m[15:04:22] [Server thread/INFO]: Done\u{1b}[0m\r";
        assert_eq!(tidy(coloured), "[15:04:22] [Server thread/INFO]: Done");
        assert!(tidy(coloured).starts_with("[15:"));
    }

    #[test]
    fn control_characters_go_but_the_tab_stays() {
        assert_eq!(tidy("a\u{7}b\tc\u{0}d"), "ab\tcd");
    }

    #[test]
    fn an_overlong_line_is_cut_and_says_so() {
        let line = tidy(&"a".repeat(MAX_LINE_BYTES * 2));
        assert_eq!(line.len(), MAX_LINE_BYTES);
        assert!(line.ends_with(TRUNCATION_MARK));
    }

    #[test]
    fn the_tail_of_a_log_is_its_end_and_not_its_start() {
        let dir = std::env::temp_dir().join(format!("craftpanel-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let path = dir.join("latest.log");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("a log");

        assert_eq!(tail_of_log(&path, 2, 1 << 20), vec!["two", "three"]);
        assert_eq!(tail_of_log(&dir.join("missing.log"), 2, 1 << 20), Vec::<String>::new());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_log_far_bigger_than_the_ring_is_read_from_its_end_only() {
        let dir = std::env::temp_dir().join(format!("craftpanel-fat-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let path = dir.join("latest.log");

        let mut log = b"\xff\xfe the first line, which must never be read\n".to_vec();
        for index in 0..20_000 {
            log.extend(format!("[15:04:22] [Server thread/INFO]: line {index}\n").as_bytes());
        }
        std::fs::write(&path, &log).expect("a log");

        let tail = tail_of_log(&path, MAX_LINES, 64 * 1024);
        let held: usize = tail.iter().map(|line| line.len() + 1).sum();
        assert!(held <= 64 * 1024, "read {held} bytes of a {} byte log", log.len());
        assert!(!tail.is_empty(), "one unreadable byte must not blank the whole retrospect");
        let last = tail.last().map(String::as_str);
        assert_eq!(last, Some("[15:04:22] [Server thread/INFO]: line 19999"));
        assert!(!tail.iter().any(|line| line.contains("never be read")));
        assert!(tail[0].starts_with("[15:04:22]"), "{}", tail[0]);

        assert_eq!(tail_of_log(&path, 3, 64 * 1024).len(), 3, "and the line ceiling holds too");
        std::fs::remove_dir_all(&dir).ok();
    }
}
