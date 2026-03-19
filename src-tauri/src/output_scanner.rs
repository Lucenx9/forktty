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
            prompt_patterns,
            prompt_emitted_for_line: false,
        }
    }

    /// Scan a chunk of PTY output. Returns any detected events.
    pub fn scan(&mut self, data: &[u8]) -> Vec<ScanEvent> {
        let mut events = Vec::new();

        // 1. Scan for OSC 133 sequences
        self.scan_osc133(data, &mut events);

        // 2. Update line buffer and check prompt patterns
        self.update_line_buffer(data, &mut events);

        events
    }

    /// Scan for OSC 133 sequences: ESC ] 133 ; <cmd> BEL
    fn scan_osc133(&mut self, data: &[u8], events: &mut Vec<ScanEvent>) {
        let prefix = b"\x1b]133;";
        if data.len() < 2 {
            return;
        }

        for i in 0..data.len() {
            if data[i..].starts_with(prefix) {
                let cmd_idx = i + prefix.len();
                if cmd_idx < data.len() {
                    match data[cmd_idx] {
                        b'A' => {
                            if !self.prompt_emitted_for_line {
                                events.push(ScanEvent::PromptDetected);
                                self.prompt_emitted_for_line = true;
                            }
                        }
                        b'C' => {
                            events.push(ScanEvent::CommandStarted);
                            self.prompt_emitted_for_line = false;
                        }
                        b'D' => {
                            let exit_code = self.parse_osc133d_exit_code(data, cmd_idx);
                            events.push(ScanEvent::CommandFinished { exit_code });
                            self.prompt_emitted_for_line = false;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Parse exit code from OSC 133 D sequence: ";D;0\x07"
    fn parse_osc133d_exit_code(&self, data: &[u8], d_idx: usize) -> Option<i32> {
        // Look for ";digits" after D
        let after_d = d_idx + 1;
        if after_d < data.len() && data[after_d] == b';' {
            let start = after_d + 1;
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
}
