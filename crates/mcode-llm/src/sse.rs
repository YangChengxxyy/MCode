//! Incremental Server-Sent Events framing shared by wire adapters.
//!
//! The framer is protocol-neutral: it handles bytes, comments, CRLF, and the
//! optional `[DONE]` sentinel while adapters interpret JSON event payloads.

/// Frames a byte stream into server-sent-event data payloads.
///
/// Events are separated by blank lines; `data:` lines join with `\n`; comment
/// and non-data fields are ignored. Arbitrary byte chunk boundaries, including
/// boundaries inside UTF-8, are accepted.
#[derive(Debug, Default)]
pub struct SseFramer {
    buf: Vec<u8>,
    data_lines: Vec<String>,
    done: bool,
}

impl SseFramer {
    /// Creates an empty framer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds raw bytes and returns all completed data payloads.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        if self.done {
            return Vec::new();
        }
        self.buf.extend_from_slice(bytes);
        let mut payloads = Vec::new();
        while let Some(position) = self.buf.iter().position(|&byte| byte == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=position).collect();
            let _ = line.pop();
            if line.last() == Some(&b'\r') {
                let _ = line.pop();
            }
            self.process_line(&line, &mut payloads);
            if self.done {
                self.buf.clear();
                break;
            }
        }
        payloads
    }

    /// Flushes a final event not terminated by a blank line.
    pub fn finish(&mut self) -> Vec<String> {
        let mut payloads = Vec::new();
        if self.done {
            return payloads;
        }
        if !self.buf.is_empty() {
            let mut line = std::mem::take(&mut self.buf);
            if line.last() == Some(&b'\r') {
                let _ = line.pop();
            }
            self.process_line(&line, &mut payloads);
        }
        self.dispatch(&mut payloads);
        payloads
    }

    /// Returns whether the `[DONE]` sentinel has been seen.
    pub fn is_done(&self) -> bool {
        self.done
    }

    fn process_line(&mut self, line: &[u8], payloads: &mut Vec<String>) {
        if line.is_empty() {
            self.dispatch(payloads);
        } else if line.starts_with(b":") {
            // SSE comment / keep-alive.
        } else if let Some(data) = line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").unwrap_or(data);
            self.data_lines
                .push(String::from_utf8_lossy(data).into_owned());
        }
    }

    fn dispatch(&mut self, payloads: &mut Vec<String>) {
        if self.data_lines.is_empty() {
            return;
        }
        let payload = self.data_lines.join("\n");
        self.data_lines.clear();
        if payload == "[DONE]" {
            self.done = true;
        } else {
            payloads.push(payload);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_comments_crlf_multiline_and_done() {
        let raw = b": keepalive\r\ndata: {\"a\":\r\ndata: 1}\r\n\r\ndata: [DONE]\r\n\r\ndata: ignored\n\n";
        let mut framer = SseFramer::new();
        let mut payloads = Vec::new();
        for chunk in raw.chunks(3) {
            payloads.extend(framer.feed(chunk));
        }
        payloads.extend(framer.finish());
        assert_eq!(payloads, vec!["{\"a\":\n1}".to_owned()]);
        assert!(framer.is_done());
    }

    #[test]
    fn flushes_unterminated_event() {
        let mut framer = SseFramer::new();
        assert!(framer.feed(b"data: final").is_empty());
        assert_eq!(framer.finish(), vec!["final".to_owned()]);
    }
}

// Rust guideline compliant 2026-08-26
