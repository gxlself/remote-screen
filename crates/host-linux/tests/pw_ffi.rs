use std::ffi::c_void;
use std::mem::{offset_of, size_of};

use host_linux::pw::{Chunk, PwBuffer};
use host_linux::pw_ffi::{
    self, PwBufferRaw, PwStreamEvents, SpaBuffer, SpaChunk, SpaData, SpaDict, SpaHook, SpaMeta,
};

#[test]
fn layouts() {
    assert_eq!(size_of::<SpaBuffer>(), 24);
    assert_eq!(offset_of!(SpaBuffer, metas), 8);
    assert_eq!(offset_of!(SpaBuffer, datas), 16);
    assert_eq!(size_of::<SpaMeta>(), 16);
    assert_eq!(offset_of!(SpaMeta, data), 8);
    assert_eq!(size_of::<SpaData>(), 40);
    assert_eq!(offset_of!(SpaData, data), 24);
    assert_eq!(offset_of!(SpaData, chunk), 32);
    assert_eq!(size_of::<SpaChunk>(), 16);
    assert_eq!(size_of::<SpaDict>(), 16);
    assert_eq!(size_of::<SpaHook>(), 48);
    assert_eq!(offset_of!(PwBufferRaw, buffer), 0);
    assert_eq!(size_of::<PwStreamEvents>(), 96);
}

#[test]
fn version_parse() {
    assert_eq!(pw_ffi::parse_version("1.0.5"), Some((1, 0, 5)));
    assert_eq!(pw_ffi::parse_version("0.3.48"), Some((0, 3, 48)));
    assert!(pw_ffi::parse_version("0.3.48").unwrap() < pw_ffi::MIN_VERSION);
    assert_eq!(pw_ffi::parse_version("1.2"), Some((1, 2, 0)));
    assert_eq!(pw_ffi::parse_version("x"), None);
}

#[test]
fn buffer_view_over_fake_struct() {
    let mut header = [0_u8; 32];
    let mut damage = [0_u8; 32];
    let mut bytes = [0_u8; 64];
    let mut chunk = SpaChunk {
        offset: 0,
        size: 48,
        stride: 16,
        flags: 0,
    };
    let mut metas = [
        SpaMeta {
            type_: host_linux::spa_pod::meta_type::HEADER,
            size: header.len() as u32,
            data: header.as_mut_ptr().cast::<c_void>(),
        },
        SpaMeta {
            type_: host_linux::spa_pod::meta_type::VIDEO_DAMAGE,
            size: damage.len() as u32,
            data: damage.as_mut_ptr().cast::<c_void>(),
        },
    ];
    let mut datas = [SpaData {
        type_: pw_ffi::data_type::MEM_FD,
        flags: 0,
        fd: 3,
        mapoffset: 0,
        maxsize: bytes.len() as u32,
        data: bytes.as_mut_ptr().cast::<c_void>(),
        chunk: &mut chunk,
    }];
    let mut buffer = SpaBuffer {
        n_metas: metas.len() as u32,
        n_datas: datas.len() as u32,
        metas: metas.as_mut_ptr(),
        datas: datas.as_mut_ptr(),
    };
    let mut raw = PwBufferRaw {
        buffer: &mut buffer,
        user_data: std::ptr::null_mut(),
        size: 0,
        requested: 0,
    };
    // SAFETY: raw points to the fake structures kept alive for this test.
    let mut view = unsafe { PwBuffer::from_raw(&mut raw) };
    assert_eq!(view.n_datas(), 1);
    assert_eq!(
        view.meta(host_linux::spa_pod::meta_type::HEADER)
            .unwrap()
            .len(),
        32
    );
    assert!(view.meta(host_linux::spa_pod::meta_type::CURSOR).is_none());
    let data = view.data(0).expect("data");
    assert_eq!(data.data_type, pw_ffi::data_type::MEM_FD);
    assert_eq!(data.valid().unwrap().len(), 48);
    assert!(view.data(1).is_none());
    view.data_mut(0).unwrap()[3] = 0xa5;
    assert_eq!(bytes[3], 0xa5);
    view.set_chunk(0, 4, 12, 32, 7);
    assert_eq!(
        view.data(0).unwrap().chunk,
        Chunk {
            offset: 4,
            size: 12,
            stride: 32,
            flags: 7,
        }
    );
    view.set_chunk(0, 70, 10, 16, 0);
    assert_eq!(view.data(0).unwrap().valid().unwrap().len(), 10);
    view.set_chunk(0, 60, 10, 16, 0);
    assert!(view.data(0).unwrap().valid().is_none());
}
