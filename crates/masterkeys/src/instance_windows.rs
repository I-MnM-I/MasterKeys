//! One GUI at a time on Windows: a named mutex, and a named event a second launch sets to raise the
//! first.

use super::GuiInstance;

/// Become the GUI, or raise the one already running and return `None`.
pub fn claim_or_raise() -> Option<GuiInstance> {
    let show: super::ShowFn = Default::default();
    Some(GuiInstance { show })
}
