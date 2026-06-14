use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

pub enum CliAction { Unknown(String) }

pub fn parse<I, S>(args: I) -> CliAction
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    CliAction::Unknown("unknown".to_string())
}

fn main() {
    let invalid_utf8 = OsStr::from_bytes(&[0xFF, 0xFF, 0xFF]);
    parse::<_, &OsStr>(&[OsStr::new("forktty"), invalid_utf8]);
}
