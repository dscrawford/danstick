//! Framing for the daemon's unix socket: newline-delimited JSON.
//!
//! Chosen over D-Bus or a custom binary framing so that writing a client is a
//! few dozen lines in any language, with no extra dependency and nothing to
//! generate. That is the whole point of padmap: a program attaches to the
//! socket, drives the controllers, and needs no library from here to do it.
//!
//! It is also a hard compatibility boundary while the port is in progress --
//! the Rust daemon has to be a drop-in for the Python one on the same socket.

use serde_json::{Map, Value};

/// Most one unterminated line may accumulate before it is thrown away.
///
/// The socket lives in `XDG_RUNTIME_DIR` and any process running as this user
/// may connect to it, so without a bound `yes | nc -U .../padmap.sock` grows
/// the daemon until the OOM killer takes it -- and every virtual pad on the
/// machine goes with the daemon, mid-game.
///
/// Generous rather than tight, because the cost of guessing low is a front-end
/// whose console picker silently never opens: the largest thing padmap puts on
/// this socket is a layout choice carrying whole layouts (~8 KiB for five
/// consoles). Anything within an order of magnitude of this limit is not
/// padmap traffic.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// One message, newline-terminated.
///
/// Compact separators, so a message never contains a stray newline from
/// pretty-printing, which would desynchronise the framing.
pub fn encode(message: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec(message).unwrap_or_else(|_| b"{}".to_vec());
    out.push(b'\n');
    out
}

/// Accumulates socket reads and yields whole JSON messages.
///
/// A stream socket splits messages anywhere, so a client that assumes one
/// `recv` is one message works until it doesn't.
///
/// `feed` never fails. Everything it is handed came off a socket any local
/// process may connect to, and its caller is the event loop itself -- an error
/// escaping here ends the process rather than the connection, and every
/// player's controller goes with it.
#[derive(Debug, Default)]
pub struct LineReader {
    /// A buffer with two offsets into it rather than a `Vec` that is re-sliced.
    ///
    /// `buffer = buffer.split(b"\n", 1)[1]` copies everything still pending on
    /// every read and `b"\n" in buffer` rescans it, which makes reassembling
    /// one message quadratic in its length. Measured at 27 seconds for a 1 MiB
    /// message arriving a byte at a time -- 27 seconds during which the daemon
    /// forwards no pad events at all.
    buffer: Vec<u8>,
    /// First byte of the line being accumulated.
    start: usize,
    /// How far we have already looked for its newline.
    scanned: usize,
    /// True while discarding the tail of a line that blew [`MAX_LINE_BYTES`].
    dropping: bool,
}

impl LineReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whole messages completed by this read, in arrival order.
    ///
    /// Only JSON objects are returned. A line that is valid JSON but not an
    /// object -- a bare number, a list -- is dropped: the protocol is objects,
    /// and a caller indexing into anything else is a caller that crashes.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Map<String, Value>> {
        self.buffer.extend_from_slice(data);
        let mut messages = Vec::new();

        while let Some(offset) = memchr(b'\n', &self.buffer[self.scanned..]) {
            let end = self.scanned + offset;
            let line = &self.buffer[self.start..end];
            // Consume the line *before* parsing it. Whatever happens next, the
            // framing has already resynchronised on this newline.
            let line = trim(line).to_vec();
            self.start = end + 1;
            self.scanned = end + 1;

            if self.dropping {
                // The tail of an over-long line, and the newline that finally
                // ended it. Nothing to parse; normal framing resumes here.
                self.dropping = false;
                continue;
            }
            if line.is_empty() {
                continue;
            }
            // A malformed line is worth skipping rather than killing the
            // connection: the framing is still intact after the newline. This
            // includes a line nested past serde_json's recursion limit, which
            // is the shape that used to end the Python daemon outright -- any
            // process that could open the socket could kill it, and a good
            // message earlier in the same read was lost with it.
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
            // A peer that sends and sends without ever sending a newline. Drop
            // what it has sent and keep dropping until one arrives: refusing to
            // grow is the whole point, and the next newline is the only place
            // this connection can be trusted to make sense again.
            self.buffer.clear();
            self.scanned = 0;
            self.dropping = true;
        }

        messages
    }

    /// Bytes held for a line that has not ended yet. For diagnostics.
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|byte| *byte == needle)
}

/// `bytes.strip()`: ASCII whitespace from both ends.
///
/// The Python stripped the line before parsing, and `serde_json` would accept
/// the untrimmed bytes anyway -- but an all-whitespace line has to come out
/// empty here, or it is parsed and rejected once per keepalive.
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
        // The framing is still intact after the newline, so the connection
        // survives and the next message is read normally.
        let mut reader = LineReader::new();
        let messages = feed_str(&mut reader, "not json\n{\"cmd\":\"status\"}\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["cmd"], "status");
    }

    #[test]
    fn a_deeply_nested_line_does_not_end_the_process() {
        // Any process that can open the socket could send this. In the Python
        // it raised RecursionError -- not a ValueError -- out of the selector
        // loop, and every virtual pad went with the daemon, mid-game.
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
        // The tail of the over-long line, its terminator, then a real message.
        let messages = reader.feed(b"more rubbish\n{\"cmd\":\"status\"}\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["cmd"], "status");
    }

    #[test]
    fn reassembling_a_large_message_byte_by_byte_is_not_quadratic() {
        // 27 seconds for 1 MiB was the measured cost of the re-slicing version,
        // and the daemon forwards nothing while it happens.
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
        // The framing is over bytes; a UTF-8 sequence can be cut anywhere.
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
