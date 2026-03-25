use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine,
};
use regex::Regex;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event_type")]
pub enum ScanEvent {
    #[serde(rename = "prompt_detected")]
    PromptDetected,
    #[serde(rename = "command_started")]
    CommandStarted,
    #[serde(rename = "command_finished")]
    CommandFinished { exit_code: Option<i32> },
    #[serde(rename = "notification")]
    Notification { title: String, body: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Osc99PayloadType {
    Title,
    Body,
    Ignore,
}

/// Scans PTY output for OSC 133 shell integration sequences and
/// Claude Code prompt patterns. Runs inline in the PTY read loop.
pub struct OutputScanner {
    line_buf: Vec<u8>,
    osc_buf: Vec<u8>,
    prompt_patterns: Vec<Regex>,
    /// Track if we already emitted PromptDetected for the current line
    /// to avoid duplicate notifications from both OSC and pattern match.
    prompt_emitted_for_line: bool,
}

impl OutputScanner {
    pub fn new() -> Self {
        let patterns = vec![
            r"^>\s*$",                 // Claude Code ">" prompt
            r"^❯\s*$",                 // Unicode prompt variant
            r"\? .+\(Y/n\)",           // Confirmation prompt
            r"\? .+:\s*$",             // Input prompt
            r"Do you want to proceed", // Permission prompt
        ];

        let prompt_patterns = patterns
            .into_iter()
            .map(|p| Regex::new(p).expect("hardcoded prompt pattern must be valid"))
            .collect();

        Self {
            line_buf: Vec::with_capacity(1024),
            osc_buf: Vec::with_capacity(64),
            prompt_patterns,
            prompt_emitted_for_line: false,
        }
    }

    /// Scan a chunk of PTY output. Returns any detected events.
    pub fn scan(&mut self, data: &[u8]) -> Vec<ScanEvent> {
        let mut events = Vec::new();

        // 1. Scan for OSC sequences (avoid allocation in the common case)
        if self.osc_buf.is_empty() {
            self.scan_osc(data, &mut events);
        } else {
            let mut combined = std::mem::take(&mut self.osc_buf);
            combined.extend_from_slice(data);
            self.scan_osc(&combined, &mut events);
        }

        // 2. Update line buffer and check prompt patterns
        self.update_line_buffer(data, &mut events);

        events
    }

    /// Scan for OSC sequences: ESC ] <number> ; <payload> BEL
    /// Dispatches on OSC number: 133 (shell integration), 9/99/777 (notifications).
    fn scan_osc(&mut self, data: &[u8], events: &mut Vec<ScanEvent>) {
        let osc_start_marker = b"\x1b]";
        if data.is_empty() {
            return;
        }

        let mut index = 0;
        while index < data.len() {
            let Some(rel_start) = find_subslice(&data[index..], osc_start_marker) else {
                break;
            };
            let start = index + rel_start;
            let after_esc_bracket = start + osc_start_marker.len();

            let Some((terminator_start, terminator_len)) =
                find_osc_terminator(&data[after_esc_bracket..])
            else {
                // Buffer partial sequence; cap at 4KB to prevent memory exhaustion
                if self.osc_buf.len() + (data.len() - start) <= 4096 {
                    self.osc_buf.extend_from_slice(&data[start..]);
                } else {
                    self.osc_buf.clear(); // discard malformed/oversized sequence
                }
                return;
            };

            let payload = &data[after_esc_bracket..after_esc_bracket + terminator_start];
            self.dispatch_osc(payload, events);
            index = after_esc_bracket + terminator_start + terminator_len;
        }

        // Check for trailing partial ESC ] in the unprocessed tail only
        let tail = &data[index..];
        if let Some(start) = trailing_partial_osc_prefix(tail, osc_start_marker) {
            let partial = &tail[start..];
            if self.osc_buf.len() + partial.len() <= 4096 {
                self.osc_buf.extend_from_slice(partial);
            } else {
                self.osc_buf.clear();
            }
        }
    }

    /// Dispatch a complete OSC payload (everything between ESC] and BEL/ST).
    fn dispatch_osc(&mut self, payload: &[u8], events: &mut Vec<ScanEvent>) {
        // OSC 133 — shell integration: "133;X..."
        if payload.starts_with(b"133;") {
            let sequence = &payload[4..];
            match sequence.first().copied() {
                Some(b'A') => {
                    if !self.prompt_emitted_for_line {
                        events.push(ScanEvent::PromptDetected);
                        self.prompt_emitted_for_line = true;
                    }
                }
                Some(b'C') => {
                    events.push(ScanEvent::CommandStarted);
                    self.prompt_emitted_for_line = false;
                }
                Some(b'D') => {
                    let exit_code = self.parse_osc133d_exit_code(sequence);
                    events.push(ScanEvent::CommandFinished { exit_code });
                    self.prompt_emitted_for_line = false;
                }
                _ => {}
            }
            return;
        }

        // OSC 9 — iTerm2 notification: "9;<text>"
        // Skip ConEmu extensions: "9;N;..." where N is a digit (e.g., progress reporting)
        if payload.starts_with(b"9;") {
            let content = &payload[2..];
            let is_conemu = content.len() >= 2 && content[0].is_ascii_digit() && content[1] == b';';
            if !is_conemu {
                if let Ok(text) = std::str::from_utf8(content) {
                    let stripped = strip_ansi(text.as_bytes());
                    let clean = String::from_utf8_lossy(&stripped).into_owned();
                    events.push(ScanEvent::Notification {
                        title: "Terminal".to_string(),
                        body: clean,
                    });
                }
            }
            return;
        }

        // OSC 99 — kitty notification protocol: "99;key=val:key=val;payload"
        if payload.starts_with(b"99;") {
            if let Ok(rest) = std::str::from_utf8(&payload[3..]) {
                // Split metadata (key=val:key=val) from payload at first unescaped semicolon
                let (metadata, body) = rest.split_once(';').unwrap_or((rest, ""));
                let (payload_type, is_base64) = parse_osc99_metadata(metadata);
                if payload_type == Osc99PayloadType::Ignore {
                    return;
                }

                let decoded = decode_osc99_payload(body, is_base64);
                let clean = String::from_utf8_lossy(&strip_ansi(&decoded)).into_owned();
                if !clean.is_empty() {
                    let title = if payload_type == Osc99PayloadType::Title {
                        clean.clone()
                    } else {
                        "Terminal".to_string()
                    };
                    events.push(ScanEvent::Notification { title, body: clean });
                }
            }
            return;
        }

        // OSC 777 — rxvt-unicode notification: "777;notify;<title>;<body>"
        if payload.starts_with(b"777;notify;") {
            if let Ok(rest) = std::str::from_utf8(&payload[11..]) {
                let (title, body) = rest.split_once(';').unwrap_or((rest, ""));
                let clean_title =
                    String::from_utf8_lossy(&strip_ansi(title.as_bytes())).into_owned();
                let clean_body = String::from_utf8_lossy(&strip_ansi(body.as_bytes())).into_owned();
                events.push(ScanEvent::Notification {
                    title: clean_title,
                    body: clean_body,
                });
            }
        }
    }

    /// Parse exit code from OSC 133 D sequence: ";D;0\x07"
    fn parse_osc133d_exit_code(&self, data: &[u8]) -> Option<i32> {
        // Look for ";digits" after D
        if data.first() == Some(&b'D') && data.get(1) == Some(&b';') {
            let start = 2;
            let mut end = start;
            while end < data.len() && data[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                if let Ok(s) = std::str::from_utf8(&data[start..end]) {
                    return s.parse().ok();
                }
            }
        }
        None
    }

    /// Update the line buffer with new data and check prompt patterns.
    fn update_line_buffer(&mut self, data: &[u8], events: &mut Vec<ScanEvent>) {
        for &byte in data {
            if byte == b'\n' || byte == b'\r' {
                // Line complete — check patterns
                if !self.line_buf.is_empty()
                    && !self.prompt_emitted_for_line
                    && self.matches_prompt_pattern()
                {
                    events.push(ScanEvent::PromptDetected);
                    self.prompt_emitted_for_line = true;
                }
                self.line_buf.clear();
                self.prompt_emitted_for_line = false;
            } else {
                self.line_buf.push(byte);
                // Cap buffer at 1KB to prevent memory growth
                if self.line_buf.len() > 1024 {
                    self.line_buf.drain(..512);
                }
            }
        }

        // Also check the current (incomplete) line — prompts don't end with \n
        if !self.line_buf.is_empty()
            && !self.prompt_emitted_for_line
            && self.matches_prompt_pattern()
        {
            events.push(ScanEvent::PromptDetected);
            self.prompt_emitted_for_line = true;
        }
    }

    /// Check if the current line buffer matches any prompt pattern.
    fn matches_prompt_pattern(&self) -> bool {
        let stripped = strip_ansi(&self.line_buf);
        let line = String::from_utf8_lossy(&stripped);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        self.prompt_patterns.iter().any(|re| re.is_match(trimmed))
    }
}

impl Default for OutputScanner {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_osc99_metadata(metadata: &str) -> (Osc99PayloadType, bool) {
    let mut payload_type = Osc99PayloadType::Body;
    let mut is_base64 = false;

    for param in metadata.split(':') {
        if let Some(value) = param.strip_prefix("p=") {
            payload_type = match value {
                "title" => Osc99PayloadType::Title,
                "body" => Osc99PayloadType::Body,
                _ => Osc99PayloadType::Ignore,
            };
        } else if param == "e=1" {
            is_base64 = true;
        }
    }

    (payload_type, is_base64)
}

fn decode_osc99_payload(payload: &str, is_base64: bool) -> Vec<u8> {
    if !is_base64 {
        return payload.as_bytes().to_vec();
    }

    let trimmed = payload.trim();
    STANDARD
        .decode(trimmed)
        .or_else(|_| STANDARD_NO_PAD.decode(trimmed))
        .unwrap_or_else(|_| payload.as_bytes().to_vec())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_osc_terminator(data: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x07 {
            return Some((i, 1));
        }
        if data[i] == 0x1b && data.get(i + 1) == Some(&b'\\') {
            return Some((i, 2));
        }
        i += 1;
    }
    None
}

fn trailing_partial_osc_prefix(data: &[u8], prefix: &[u8]) -> Option<usize> {
    // Only the last (prefix.len - 1) bytes can be a partial prefix
    let scan_start = data.len().saturating_sub(prefix.len() - 1);
    for start in (scan_start..data.len()).rev() {
        let suffix = &data[start..];
        if prefix.starts_with(suffix) {
            return Some(start);
        }
    }
    None
}

/// Strip ANSI escape sequences from byte data.
fn strip_ansi(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x1b {
            i += 1;
            if i >= data.len() {
                break;
            }
            if data[i] == b'[' {
                // CSI: skip until final byte (0x40-0x7E)
                i += 1;
                while i < data.len() && !(0x40..=0x7E).contains(&data[i]) {
                    i += 1;
                }
                if i < data.len() {
                    i += 1;
                }
            } else if data[i] == b']' {
                // OSC: skip until BEL or ST
                i += 1;
                while i < data.len() && data[i] != 0x07 {
                    if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                if i < data.len() && data[i] == 0x07 {
                    i += 1;
                }
            } else {
                // Other escape — skip one byte
                i += 1;
            }
        } else {
            result.push(data[i]);
            i += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osc133_prompt() {
        let mut scanner = OutputScanner::new();
        let data = b"\x1b]133;A\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ScanEvent::PromptDetected));
    }

    #[test]
    fn test_osc133_command_lifecycle() {
        let mut scanner = OutputScanner::new();

        let events = scanner.scan(b"\x1b]133;C\x07");
        assert!(matches!(events[0], ScanEvent::CommandStarted));

        let events = scanner.scan(b"\x1b]133;D;0\x07");
        assert!(matches!(
            events[0],
            ScanEvent::CommandFinished { exit_code: Some(0) }
        ));
    }

    #[test]
    fn test_claude_code_prompt_pattern() {
        let mut scanner = OutputScanner::new();
        let data = b"> \n";
        let events = scanner.scan(data);
        assert!(events
            .iter()
            .any(|e| matches!(e, ScanEvent::PromptDetected)));
    }

    #[test]
    fn test_strip_ansi() {
        let data = b"\x1b[32m> \x1b[0m";
        let stripped = strip_ansi(data);
        assert_eq!(&stripped, b"> ");
    }

    #[test]
    fn test_no_duplicate_prompt_events() {
        let mut scanner = OutputScanner::new();
        // OSC 133 A + pattern match should only emit once
        let data = b"\x1b]133;A\x07> ";
        let events = scanner.scan(data);
        let prompt_count = events
            .iter()
            .filter(|e| matches!(e, ScanEvent::PromptDetected))
            .count();
        assert_eq!(prompt_count, 1);
    }

    #[test]
    fn test_osc133_prompt_split_across_chunks() {
        let mut scanner = OutputScanner::new();

        let events = scanner.scan(b"\x1b]13");
        assert!(events.is_empty());

        let events = scanner.scan(b"3;A\x07");
        assert!(events
            .iter()
            .any(|e| matches!(e, ScanEvent::PromptDetected)));
    }

    #[test]
    fn test_osc133_exit_code_split_across_chunks() {
        let mut scanner = OutputScanner::new();

        let events = scanner.scan(b"\x1b]133;D;17");
        assert!(events.is_empty());

        let events = scanner.scan(b"\x07");
        assert!(matches!(
            events[0],
            ScanEvent::CommandFinished {
                exit_code: Some(17)
            }
        ));
    }

    #[test]
    fn test_osc9_notification() {
        let mut scanner = OutputScanner::new();
        let data = b"\x1b]9;Hello World\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "Terminal");
                assert_eq!(body, "Hello World");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_osc99_notification() {
        let mut scanner = OutputScanner::new();
        // kitty format: "99;key=val:key=val;payload"
        let data = b"\x1b]99;i=myid;Task complete\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "Terminal");
                assert_eq!(body, "Task complete");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_osc99_title_payload() {
        let mut scanner = OutputScanner::new();
        // When p=title, the payload IS the title
        let data = b"\x1b]99;p=title;My Title\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "My Title");
                assert_eq!(body, "My Title");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_osc99_base64_payload_decoded() {
        let mut scanner = OutputScanner::new();
        let data = b"\x1b]99;e=1;SGVsbG8\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "Terminal");
                assert_eq!(body, "Hello");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_osc99_close_payload_is_skipped() {
        let mut scanner = OutputScanner::new();
        let data = b"\x1b]99;i=myid:p=close;untracked\x07";
        let events = scanner.scan(data);
        assert!(
            events.is_empty(),
            "kitty close reports should not emit notifications"
        );
    }

    #[test]
    fn test_osc99_query_payload_is_skipped() {
        let mut scanner = OutputScanner::new();
        let data = b"\x1b]99;i=myid:p=?;p=title:s=silent\x07";
        let events = scanner.scan(data);
        assert!(
            events.is_empty(),
            "kitty query responses should not emit notifications"
        );
    }

    #[test]
    fn test_osc9_conemu_skipped() {
        let mut scanner = OutputScanner::new();
        // ConEmu progress: "9;4;50" — should be skipped
        let data = b"\x1b]9;4;50\x07";
        let events = scanner.scan(data);
        assert!(
            events.is_empty(),
            "ConEmu progress should not emit notification"
        );
    }

    #[test]
    fn test_osc777_strips_ansi() {
        let mut scanner = OutputScanner::new();
        let data = b"\x1b]777;notify;\x1b[1mBold Title\x1b[0m;\x1b[32mGreen body\x1b[0m\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "Bold Title");
                assert_eq!(body, "Green body");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_osc777_notification() {
        let mut scanner = OutputScanner::new();
        let data = b"\x1b]777;notify;Build Status;Build succeeded\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "Build Status");
                assert_eq!(body, "Build succeeded");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_osc_notification_split_across_chunks() {
        let mut scanner = OutputScanner::new();

        let events = scanner.scan(b"\x1b]9;Hel");
        assert!(events.is_empty());

        let events = scanner.scan(b"lo\x07");
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "Terminal");
                assert_eq!(body, "Hello");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_osc133_still_works_after_refactor() {
        let mut scanner = OutputScanner::new();
        // Verify all OSC 133 variants still work with the new general scanner
        let events = scanner.scan(b"\x1b]133;A\x07");
        assert!(matches!(events[0], ScanEvent::PromptDetected));

        let events = scanner.scan(b"\x1b]133;C\x07");
        assert!(matches!(events[0], ScanEvent::CommandStarted));

        let events = scanner.scan(b"\x1b]133;D;0\x07");
        assert!(matches!(
            events[0],
            ScanEvent::CommandFinished { exit_code: Some(0) }
        ));
    }

    #[test]
    fn test_osc_with_st_terminator() {
        let mut scanner = OutputScanner::new();
        // ST terminator is ESC backslash
        let data = b"\x1b]9;Hello\x1b\\";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ScanEvent::Notification { title, body } => {
                assert_eq!(title, "Terminal");
                assert_eq!(body, "Hello");
            }
            _ => panic!("Expected Notification event"),
        }
    }

    #[test]
    fn test_consecutive_st_terminated_osc_sequences() {
        let mut scanner = OutputScanner::new();
        // Two OSC 9 notifications with ST terminators back-to-back
        let data = b"\x1b]9;First\x1b\\\x1b]9;Second\x07";
        let events = scanner.scan(data);
        assert_eq!(events.len(), 2);
        match &events[0] {
            ScanEvent::Notification { body, .. } => assert_eq!(body, "First"),
            _ => panic!("Expected first Notification"),
        }
        match &events[1] {
            ScanEvent::Notification { body, .. } => assert_eq!(body, "Second"),
            _ => panic!("Expected second Notification"),
        }
    }
}
