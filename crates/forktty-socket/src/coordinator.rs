//! Serializes mutations that create, close, or replace terminal surfaces.

#[derive(Default, Debug)]
pub(crate) struct SocketCoordinator {
    pub(crate) surface_set: tokio::sync::Mutex<()>,
}
