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
            .filter_map(|p| Regex::new(p).ok())
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

        // 1. Scan for OSC 133 sequences (avoid allocation in the common case)
        if self.osc_buf.is_empty() {
            self.scan_osc133(data, &mut events);
        } else {
            let mut combined = std::mem::take(&mut self.osc_buf);
            combined.extend_from_slice(data);
            self.scan_osc133(&combined, &mut events);
        }

        // 2. Update line buffer and check prompt patterns
        self.update_line_buffer(data, &mut events);

        events
    }

    /// Scan for OSC 133 sequences: ESC ] 133 ; <cmd> BEL
    fn scan_osc133(&mut self, data: &[u8], events: &mut Vec<ScanEvent>) {
        let prefix = b"\x1b]133;";
        if data.is_empty() {
            return;
        }

        let mut index = 0;
        while index < data.len() {
            let Some(rel_start) = find_subslice(&data[index..], prefix) else {
                break;
            };
            let start = index + rel_start;
            let sequence_start = start + prefix.len();

            let Some((terminator_start, terminator_len)) =
                find_osc_terminator(&data[sequence_start..])
            else {
                // Buffer partial sequence; cap at 4KB to prevent memory exhaustion
                if self.osc_buf.len() + (data.len() - start) <= 4096 {
                    self.osc_buf.extend_from_slice(&data[start..]);
                } else {
                    self.osc_buf.clear(); // discard malformed/oversized sequence
                }
                return;
            };

            let sequence_end = sequence_start + terminator_start;
            let sequence = &data[sequence_start..sequence_end];

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

            index = sequence_end + terminator_len;
        }

        if let Some(start) = trailing_partial_osc_prefix(data, prefix) {
            let partial = &data[start..];
            if self.osc_buf.len() + partial.len() <= 4096 {
                self.osc_buf.extend_from_slice(partial);
            } else {
                self.osc_buf.clear();
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
}
