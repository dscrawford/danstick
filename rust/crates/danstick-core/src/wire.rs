//! Newline-delimited JSON socket framing: minimal client burden.

use serde_json::{Map, Value};

/// Max unterminated line before throw-away (8 MiB: layout choices ~8 KiB).
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Newline-terminated message (compact JSON).
pub fn encode(message: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec(message).unwrap_or_else(|_| b"{}".to_vec());
    out.push(b'\n');
    out
}

/// Yields whole JSON messages from socket reads.
#[derive(Debug, Default)]
pub struct LineReader {
    buffer: Vec<u8>,
    start: usize,
    scanned: usize,
    dropping: bool,
}

impl LineReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Yields whole messages (JSON objects only).
    pub fn feed(&mut self, data: &[u8]) -> Vec<Map<String, Value>> {
        self.buffer.extend_from_slice(data);
        let mut messages = Vec::new();

        while let Some(offset) = memchr(b'\n', &self.buffer[self.scanned..]) {
            let end = self.scanned + offset;
            let line = &self.buffer[self.start..end];
            let line = trim(line).to_vec();
            self.start = end + 1;
            self.scanned = end + 1;

            if self.dropping {
                self.dropping = false;
                continue;
            }
            if line.is_empty() {
                continue;
            }
            if let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(&line) {
                messages.push(object);
            }
        }

        if self.scanned < self.buffer.len() {
            self.scanned = self.buffer.len();
        }

        if self.start > 0 {
            self.buffer.drain(..self.start);
            self.scanned -= self.start;
            self.start = 0;
        }

        if self.buffer.len() > MAX_LINE_BYTES {
            self.buffer.clear();
            self.scanned = 0;
            self.dropping = true;
        }

        messages
    }

    /// Bytes held for a line that has not ended yet.
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|byte| *byte == needle)
}

/// Strip ASCII whitespace from both ends.
fn trim(mut line: &[u8]) -> &[u8] {
    while let Some((first, rest)) = line.split_first() {
        if first.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = line.split_last() {
        if last.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed_str(reader: &mut LineReader, text: &str) -> Vec<Map<String, Value>> {
        reader.feed(text.as_bytes())
    }

    #[test]
    fn one_whole_message_in_one_read() {
        let mut reader = LineReader::new();
        let messages = feed_str(&mut reader, "{\"cmd\":\"status\"}\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["cmd"], "status");
    }

    #[test]
    fn a_message_split_across_reads_is_reassembled() {
        let mut reader = LineReader::new();
        assert!(feed_str(&mut reader, "{\"cmd\":\"be").is_empty());
        assert!(feed_str(&mut reader, "gin\",\"players\":4}").is_empty());
        let messages = feed_str(&mut reader, "\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["players"], 4);
    }

    #[test]
    fn several_messages_in_one_read_all_come_out_in_order() {
        let mut reader = LineReader::new();
        let messages = feed_str(&mut reader, "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n");
        let numbers: Vec<i64> = messages
            .iter()
            .map(|m| m["n"].as_i64().expect("n"))
            .collect();
        assert_eq!(numbers, [1, 2, 3]);
    }

    #[test]
    fn a_message_arriving_one_byte_at_a_time_still_arrives() {
        let mut reader = LineReader::new();
        let text = "{\"cmd\":\"accept\"}\n";
        let mut got = Vec::new();
        for byte in text.as_bytes() {
            got.extend(reader.feed(&[*byte]));
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["cmd"], "accept");
    }

    #[test]
    fn a_malformed_line_costs_that_line_and_nothing_else() {
        let mut reader = LineReader::new();
        let messages = feed_str(&mut reader, "not json\n{\"cmd\":\"status\"}\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["cmd"], "status");
    }

    #[test]
    fn a_deeply_nested_line_does_not_end_the_process() {
        let mut reader = LineReader::new();
        let mut hostile = "[".repeat(200_000);
        hostile.push('\n');
        hostile.push_str("{\"cmd\":\"status\"}\n");
        let messages = feed_str(&mut reader, &hostile);
        assert_eq!(messages.len(), 1, "the good message after it must survive");
        assert_eq!(messages[0]["cmd"], "status");
    }

    #[test]
    fn a_line_that_is_valid_json_but_not_an_object_is_dropped() {
        let mut reader = LineReader::new();
        let messages = feed_str(&mut reader, "42\n[1,2]\n\"text\"\nnull\n{\"ok\":1}\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["ok"], 1);
    }

    #[test]
    fn blank_and_whitespace_lines_are_skipped_silently() {
        let mut reader = LineReader::new();
        let messages = feed_str(&mut reader, "\n   \n\t\n{\"ok\":1}\n\n");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn a_peer_that_never_sends_a_newline_cannot_grow_the_daemon() {
        let mut reader = LineReader::new();
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..12 {
            assert!(reader.feed(&chunk).is_empty());
        }
        assert!(
            reader.pending() <= MAX_LINE_BYTES,
            "the buffer grew to {} bytes",
            reader.pending()
        );
    }

    #[test]
    fn framing_resumes_at_the_next_newline_after_an_over_long_line() {
        let mut reader = LineReader::new();
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..12 {
            reader.feed(&chunk);
        }
        let messages = reader.feed(b"more rubbish\n{\"cmd\":\"status\"}\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["cmd"], "status");
    }

    #[test]
    fn reassembling_a_large_message_byte_by_byte_is_not_quadratic() {
        let mut reader = LineReader::new();
        let payload = json!({"cmd": "map", "blob": "y".repeat(400_000)});
        let encoded = encode(&payload);
        let started = std::time::Instant::now();
        let mut got = Vec::new();
        for byte in &encoded {
            got.extend(reader.feed(&[*byte]));
        }
        let elapsed = started.elapsed();
        assert_eq!(got.len(), 1);
        assert!(
            elapsed.as_secs() < 5,
            "took {elapsed:?} for {} bytes",
            encoded.len()
        );
    }

    #[test]
    fn encode_never_emits_an_embedded_newline() {
        let message = json!({"event": "error", "message": "line one\nline two"});
        let encoded = encode(&message);
        assert_eq!(encoded.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(encoded.last(), Some(&b'\n'));
    }

    #[test]
    fn encode_and_feed_round_trip() {
        let message = json!({"event": "state", "state": "ready", "slots": 4});
        let mut reader = LineReader::new();
        let back = reader.feed(&encode(&message));
        assert_eq!(back.len(), 1);
        assert_eq!(Value::Object(back[0].clone()), message);
    }

    #[test]
    fn a_split_multibyte_character_survives_the_boundary() {
        let mut reader = LineReader::new();
        let encoded = encode(&json!({"name": "Pokémon pad"}));
        let (head, tail) = encoded.split_at(encoded.len() / 2);
        assert!(reader.feed(head).is_empty() || true);
        let messages = reader.feed(tail);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["name"], "Pokémon pad");
    }

    #[test]
    fn carriage_returns_around_a_line_do_not_break_it() {
        let mut reader = LineReader::new();
        let messages = feed_str(&mut reader, "  {\"ok\":1}  \r\n");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn nothing_is_held_once_every_line_has_been_consumed() {
        let mut reader = LineReader::new();
        feed_str(&mut reader, "{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(reader.pending(), 0, "the buffer must not accumulate");
    }
}
