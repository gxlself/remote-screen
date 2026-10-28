//! 虚拟显示器的 RsCore 入口与事件映射（M3 · V1）。
use super::{
    api_sessions::with_handle,
    dto_core::{CoreEvent, SessionDisplay, VdisplayStatus},
    error::CoreError,
    runtime::Core,
};
use rsp_core::pb;
use serde::Deserialize;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

static NEXT_REQUEST: AtomicU32 = AtomicU32::new(1);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VirtualDisplayOpts {
    width_px: u32,
    height_px: u32,
    #[serde(default)]
    scale: Option<f32>,
    #[serde(default)]
    refresh_hz: Option<u32>,
    #[serde(default)]
    select: Option<bool>,
}

/// opts_json 为 TS `VirtualDisplayOpts` 的 JSON；返回本次 request_id。
pub fn create(core: &Arc<Core>, id: u64, opts_json: &str) -> Result<u32, CoreError> {
    let opts: VirtualDisplayOpts = serde_json::from_str(opts_json)
        .map_err(|e| CoreError::internal(format!("invalid virtual display options: {e}")))?;
    let mut request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    if request_id == 0 {
        request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    }
    with_handle(core, id, |handle| {
        handle.create_virtual_display(pb::CreateVirtualDisplay {
            request_id,
            width_px: opts.width_px,
            height_px: opts.height_px,
            scale: opts.scale.unwrap_or(0.0),
            refresh_hz: opts.refresh_hz.unwrap_or(0),
            select: opts.select.unwrap_or(false),
        });
        Ok(())
    })?;
    Ok(request_id)
}

pub fn release(core: &Arc<Core>, id: u64, display_id: u32) -> Result<(), CoreError> {
    with_handle(core, id, |handle| {
        handle.release_virtual_display(display_id);
        Ok(())
    })
}

/// SessionEvent::VirtualDisplay → CoreEvent::SessionVdisplay。
pub fn result_event(session_id: &str, result: &pb::VirtualDisplayResult) -> Option<CoreEvent> {
    use pb::virtual_display_result::Status;

    let status = match Status::try_from(result.status) {
        Ok(Status::Ok) => VdisplayStatus::Ok,
        Ok(Status::Unsupported) => VdisplayStatus::Unsupported,
        Ok(Status::NotAllowed) => VdisplayStatus::NotAllowed,
        Ok(Status::Limit) => VdisplayStatus::Limit,
        Ok(Status::BadRequest) => VdisplayStatus::BadRequest,
        Ok(Status::Failed | Status::Unspecified) | Err(_) => VdisplayStatus::Failed,
    };
    let display = if status == VdisplayStatus::Ok {
        result.display.as_ref().map(SessionDisplay::from)
    } else {
        None
    };
    Some(CoreEvent::SessionVdisplay {
        session_id: session_id.to_owned(),
        request_id: result.request_id,
        status,
        display,
        message: (!result.message.is_empty()).then(|| result.message.clone()),
    })
}
