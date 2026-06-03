#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhosttyEvent {
    Bell,
    TitleChanged(String),
    PtyWrite(Vec<u8>),
    VisibleContentChanged,
    ChildExit { status: i32 },
    Metadata(TerminalMetadataEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalMetadataEvent {
    Osc9 { payload: String },
    Osc99 { payload: String },
}
