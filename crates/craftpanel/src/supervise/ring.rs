use std::collections::VecDeque;

use craftpanel_proto::{OutputStream, SupervisorMessage};

pub struct Ring {
    lines: VecDeque<(u64, String, OutputStream)>,
    capacity: usize,
    next_seq: u64,
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self { lines: VecDeque::with_capacity(capacity.min(256)), capacity, next_seq: 1 }
    }

    pub fn push(&mut self, line: &str, stream: OutputStream) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;

        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back((seq, line.to_owned(), stream));
        seq
    }

    pub fn replay(&self) -> Vec<SupervisorMessage> {
        self.lines
            .iter()
            .map(|(seq, line, stream)| SupervisorMessage::Output {
                seq: *seq,
                line: line.clone(),
                stream: *stream,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_numbers_keep_counting_past_the_capacity() {
        let mut ring = Ring::new(3);
        for i in 0..5 {
            ring.push(&format!("line {i}"), OutputStream::Stdout);
        }

        let replayed = ring.replay();
        assert_eq!(replayed.len(), 3);

        let seqs: Vec<u64> = replayed
            .iter()
            .map(|m| match m {
                SupervisorMessage::Output { seq, .. } => *seq,
                other => panic!("expected output, got {other:?}"),
            })
            .collect();
        assert_eq!(seqs, vec![3, 4, 5]);
    }

    #[test]
    fn replay_keeps_the_newest_lines() {
        let mut ring = Ring::new(2);
        ring.push("old", OutputStream::Stdout);
        ring.push("newer", OutputStream::Stdout);
        ring.push("newest", OutputStream::Stderr);

        let lines: Vec<String> = ring
            .replay()
            .into_iter()
            .map(|m| match m {
                SupervisorMessage::Output { line, .. } => line,
                other => panic!("expected output, got {other:?}"),
            })
            .collect();
        assert_eq!(lines, vec!["newer", "newest"]);
    }
}
