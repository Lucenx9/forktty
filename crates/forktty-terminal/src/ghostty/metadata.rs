use super::events::TerminalMetadataEvent;

const MAX_METADATA_PAYLOAD_BYTES: usize = 16 * 1024;

#[derive(Debug, Default)]
pub struct MetadataParser {
    state: ParserState,
    code: Vec<u8>,
    payload: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    #[default]
    Ground,
    Escape,
    OscCode,
    OscPayload,
    OscEscape,
}

impl MetadataParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TerminalMetadataEvent> {
        let mut events = Vec::new();
        for &byte in bytes {
            if let Some(event) = self.next(byte) {
                events.push(event);
            }
        }
        events
    }

    fn next(&mut self, byte: u8) -> Option<TerminalMetadataEvent> {
        match self.state {
            ParserState::Ground => {
                if byte == 0x1b {
                    self.state = ParserState::Escape;
                }
            }
            ParserState::Escape => {
                if byte == b']' {
                    self.code.clear();
                    self.payload.clear();
                    self.state = ParserState::OscCode;
                } else if byte != 0x1b {
                    self.state = ParserState::Ground;
                }
            }
            ParserState::OscCode => match byte {
                b'0'..=b'9' if self.code.len() < 3 => self.code.push(byte),
                b';' => self.state = ParserState::OscPayload,
                0x07 => return self.finish(),
                0x1b => self.state = ParserState::OscEscape,
                _ => self.reset(),
            },
            ParserState::OscPayload => match byte {
                0x07 => return self.finish(),
                0x1b => self.state = ParserState::OscEscape,
                _ => {
                    if self.payload.len() >= MAX_METADATA_PAYLOAD_BYTES {
                        self.reset();
                    } else {
                        self.payload.push(byte);
                    }
                }
            },
            ParserState::OscEscape => {
                if byte == b'\\' {
                    return self.finish();
                }
                if self.payload.len() + 1 >= MAX_METADATA_PAYLOAD_BYTES {
                    self.reset();
                } else {
                    self.payload.push(0x1b);
                    self.payload.push(byte);
                    self.state = ParserState::OscPayload;
                }
            }
        }
        None
    }

    fn finish(&mut self) -> Option<TerminalMetadataEvent> {
        let code = std::str::from_utf8(&self.code).ok()?.to_string();
        let payload = String::from_utf8_lossy(&self.payload).to_string();
        self.reset();
        match code.as_str() {
            "9" => Some(TerminalMetadataEvent::Osc9 { payload }),
            "99" => Some(TerminalMetadataEvent::Osc99 { payload }),
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.state = ParserState::Ground;
        self.code.clear();
        self.payload.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_osc_99_metadata() {
        let mut parser = MetadataParser::new();
        assert!(parser.feed(b"\x1b]99;status=ready").is_empty());

        let events = parser.feed(b";label=Build\x07");

        assert_eq!(
            events,
            vec![TerminalMetadataEvent::Osc99 {
                payload: "status=ready;label=Build".to_string()
            }]
        );
    }

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// Deterministic randomized sweep: malformed input must never panic, and
    /// feeding byte-at-a-time must produce the same events as one buffer
    /// (the parser is fed from arbitrary PTY chunk boundaries).
    #[test]
    fn random_input_never_panics_and_chunking_is_invariant() {
        // Biased toward the parser's structural bytes so random inputs
        // actually reach the OSC states instead of staying in Ground.
        const POOL: &[u8] = &[0x1b, b']', b'9', b'0', b';', 0x07, b'\\', b'a', 0xff, 0x00];
        let mut rng = 0x5eed_2026_0610_f00du64;
        for _ in 0..2_000 {
            let len = (xorshift(&mut rng) % 256) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    let roll = xorshift(&mut rng);
                    if roll.is_multiple_of(4) {
                        (roll >> 8) as u8
                    } else {
                        POOL[(roll >> 8) as usize % POOL.len()]
                    }
                })
                .collect();

            let whole = MetadataParser::new().feed(&bytes);
            let mut parser = MetadataParser::new();
            let mut byte_at_a_time = Vec::new();
            for &byte in &bytes {
                byte_at_a_time.extend(parser.feed(&[byte]));
            }
            assert_eq!(
                whole, byte_at_a_time,
                "chunking changed events for {bytes:?}"
            );
        }
    }

    #[test]
    fn oversize_payload_is_dropped_without_panic() {
        let mut parser = MetadataParser::new();
        let mut input = b"\x1b]9;".to_vec();
        input.extend(std::iter::repeat_n(b'a', MAX_METADATA_PAYLOAD_BYTES + 1024));
        input.push(0x07);
        assert!(parser.feed(&input).is_empty());

        // Same overflow through the OscEscape arm, which appends two bytes
        // per input byte and has its own cap check.
        let mut parser = MetadataParser::new();
        let mut input = b"\x1b]9;".to_vec();
        for _ in 0..MAX_METADATA_PAYLOAD_BYTES {
            input.extend_from_slice(b"\x1bA");
        }
        input.push(0x07);
        assert!(parser.feed(&input).is_empty());

        // The parser must still work after an overflow reset.
        let events = parser.feed(b"\x1b]9;ok\x07");
        assert_eq!(
            events,
            vec![TerminalMetadataEvent::Osc9 {
                payload: "ok".to_string()
            }]
        );
    }

    #[test]
    fn parses_osc_9_metadata_with_st_terminator() {
        let mut parser = MetadataParser::new();

        let events = parser.feed(b"\x1b]9;notify=done\x1b\\");

        assert_eq!(
            events,
            vec![TerminalMetadataEvent::Osc9 {
                payload: "notify=done".to_string()
            }]
        );
    }
}
