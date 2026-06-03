#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhosttyEvent {
    Bell,
    TitleChanged(String),
    PtyWrite(Vec<u8>),
    VisibleContentChanged,
}
