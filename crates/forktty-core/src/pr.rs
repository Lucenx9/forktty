//! Parsing of `gh pr view --json number,state,isDraft,url` output into the
//! compact PR descriptor shown in the sidebar.
//!
//! The `gh` invocation itself lives in the UI layer (it spawns a process and
//! makes a network call); this module is the pure, testable half: given the
//! JSON `gh` prints, produce a [`PrInfo`], or `None` when the payload is not a
//! usable PR record.

use serde::{Deserialize, Serialize};

/// State of a linked pull request, normalized from `gh`'s `state` + `isDraft`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
    /// Any state `gh` reports that we do not model explicitly.
    Other(String),
}

impl PrState {
    /// Short uppercase label for the sidebar (e.g. `OPEN`, `DRAFT`).
    pub fn label(&self) -> &str {
        match self {
            PrState::Open => "OPEN",
            PrState::Draft => "DRAFT",
            PrState::Merged => "MERGED",
            PrState::Closed => "CLOSED",
            PrState::Other(value) => value,
        }
    }
}

/// Linked pull request for a workspace's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub state: PrState,
    pub url: String,
}

impl PrInfo {
    /// Sidebar chip such as `#42 OPEN`.
    pub fn summary(&self) -> String {
        format!("#{} {}", self.number, self.state.label())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrView {
    number: Option<u64>,
    state: Option<String>,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    url: String,
}

/// Parse the JSON object printed by `gh pr view --json number,state,isDraft,url`.
///
/// Returns `None` when the payload is not valid JSON, lacks a PR number, or is
/// an empty object (which `gh` does not emit, but external callers might).
pub fn parse_pr_view(json: &str) -> Option<PrInfo> {
    let view: GhPrView = serde_json::from_str(json.trim()).ok()?;
    let number = view.number?;
    let state = match view.state.as_deref() {
        Some("OPEN") if view.is_draft => PrState::Draft,
        Some("OPEN") => PrState::Open,
        Some("MERGED") => PrState::Merged,
        Some("CLOSED") => PrState::Closed,
        Some(other) => PrState::Other(other.to_string()),
        None => return None,
    };
    Some(PrInfo {
        number,
        state,
        url: view.url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_pr() {
        let json = r#"{"number":42,"state":"OPEN","isDraft":false,"url":"https://github.com/o/r/pull/42"}"#;
        let pr = parse_pr_view(json).unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.summary(), "#42 OPEN");
    }

    #[test]
    fn open_with_is_draft_becomes_draft() {
        let json = r#"{"number":7,"state":"OPEN","isDraft":true,"url":"u"}"#;
        assert_eq!(parse_pr_view(json).unwrap().state, PrState::Draft);
    }

    #[test]
    fn maps_merged_and_closed() {
        let merged = r#"{"number":1,"state":"MERGED","isDraft":false,"url":"u"}"#;
        assert_eq!(parse_pr_view(merged).unwrap().state, PrState::Merged);
        let closed = r#"{"number":2,"state":"CLOSED","isDraft":false,"url":"u"}"#;
        assert_eq!(parse_pr_view(closed).unwrap().state, PrState::Closed);
    }

    #[test]
    fn unknown_state_is_preserved() {
        let json = r#"{"number":3,"state":"LOCKED","isDraft":false,"url":"u"}"#;
        assert_eq!(
            parse_pr_view(json).unwrap().state,
            PrState::Other("LOCKED".to_string())
        );
        assert_eq!(parse_pr_view(json).unwrap().summary(), "#3 LOCKED");
    }

    #[test]
    fn rejects_missing_number_or_garbage() {
        assert!(parse_pr_view(r#"{"state":"OPEN"}"#).is_none());
        assert!(parse_pr_view("not json").is_none());
        assert!(parse_pr_view("{}").is_none());
    }
}
