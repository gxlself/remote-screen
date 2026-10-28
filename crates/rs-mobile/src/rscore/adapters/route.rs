//! Input routing for adapter ids.

use std::sync::Arc;

use rs_vnc::proto::to_latin1;
use rsp_core::hid::{InputMsg, Mods};

use super::driver::DriverInput;
use super::dto::AdapterProtocol;
use super::session;
use crate::rscore::api_sessions::parse_enum;
use crate::rscore::combos::combo_inputs;
use crate::rscore::dto_core::SessionCombo;
use crate::rscore::error::CoreError;
use crate::rscore::runtime::{core, Core};

fn unknown_session() -> CoreError {
    CoreError::internal("unknown adapter session")
}

/// From rs_input_* (values already validated). Unknown id → ignored.
pub fn input(id: u64, msg: InputMsg) {
    if let Ok(c) = core() {
        let _ = session::send_input(&c, id, DriverInput::Hid(msg));
    }
}

pub fn key(core: &Arc<Core>, id: u64, usage: u16, down: bool, mods: u8) -> Result<(), CoreError> {
    session::send_input(
        core,
        id,
        DriverInput::Hid(InputMsg::Key {
            usage,
            down,
            mods: Mods::from_bits(mods),
        }),
    )
}

pub fn text(core: &Arc<Core>, id: u64, text: &str) -> Result<(), CoreError> {
    session::send_input(core, id, DriverInput::Hid(InputMsg::Text(text.to_owned())))
}

pub fn combo(core: &Arc<Core>, id: u64, combo: &str) -> Result<(), CoreError> {
    session::entry(core, id).ok_or_else(unknown_session)?;
    let combo: SessionCombo = parse_enum(combo)?;
    for input in combo_inputs(combo) {
        session::send_input(core, id, DriverInput::Hid(input))?;
    }
    Ok(())
}

pub fn clipboard_send(core: &Arc<Core>, id: u64, text: &str) -> Result<(), CoreError> {
    let entry = session::entry(core, id).ok_or_else(unknown_session)?;
    if text.is_empty() {
        return Err(CoreError::unsupported("clipboard text must not be empty"));
    }
    if entry.protocol() == AdapterProtocol::Vnc && to_latin1(text).is_none() {
        return Err(CoreError::unsupported("clipboard text is not Latin-1"));
    }
    session::send_input(core, id, DriverInput::Clipboard(text.to_owned()))
}

pub fn close(core: &Arc<Core>, id: u64) {
    session::close(core, id);
}
