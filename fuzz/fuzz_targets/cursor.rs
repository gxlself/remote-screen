#![no_main]

use libfuzzer_sys::fuzz_target;
use prost::Message;
use rsp_core::cursor::{ShapeCache, SHAPE_CACHE_CAPACITY};
use rsp_core::pb::{cursor_msg::Msg, CursorMsg};
use rsp_core::wire::FrameDecoder;

fuzz_target!(|input: &[u8]| {
    let mut frames = FrameDecoder::new();
    frames.push(input);
    let mut cache = ShapeCache::new();

    while let Ok(Some(payload)) = frames.next_frame() {
        let Ok(message) = CursorMsg::decode(payload.as_slice()) else {
            continue;
        };
        match message.msg {
            Some(Msg::Shape(shape)) => {
                cache.insert(shape);
            }
            Some(Msg::Use(id)) => {
                let _ = cache.get(id);
            }
            Some(Msg::Hidden(_)) | None => {}
        }
        assert!(
            cache.len() <= SHAPE_CACHE_CAPACITY,
            "shape cache grew to {} entries",
            cache.len()
        );
    }
});
