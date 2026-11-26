//! The tray on Windows.

use super::TrayModel;
use crate::client::Command;
use tokio::sync::mpsc::UnboundedSender;

/// The running tray; `update` redraws it when the model changed.
pub struct TrayHandle;

impl TrayHandle {
    pub fn update(&mut self, _model: TrayModel) {}
}

/// Put the icon in the system tray; `None` if there is no tray.
pub fn spawn_tray(
    _commands: UnboundedSender<Command>,
    _show: impl Fn() + Send + Sync + 'static,
    _quit: impl Fn() + Send + Sync + 'static,
) -> Option<TrayHandle> {
    None
}
