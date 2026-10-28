mod support;

use rs_mobile::rscore::{
    ffi_term::{rs_term_attach, rs_term_detach, rs_term_input, rs_term_resize},
    runtime::{install_core, take_core},
    tables::{SshTable, TermInput, TermTap},
};
use std::{
    ffi::c_void,
    ptr, slice,
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
};

#[derive(Default)]
struct Output {
    chunks: Mutex<Vec<Option<Vec<u8>>>>,
    threads: Mutex<Vec<ThreadId>>,
}
extern "C" fn output(ctx: *mut c_void, buf: *const u8, len: usize) {
    // SAFETY: ctx stays alive through detach; callback data is borrowed only
    // while this function copies it. NULL is the close marker.
    unsafe {
        let out = &*ctx.cast::<Output>();
        let bytes = if buf.is_null() {
            None
        } else {
            Some(slice::from_raw_parts(buf, len).to_vec())
        };
        out.chunks.lock().unwrap().push(bytes);
        out.threads.lock().unwrap().push(thread::current().id());
    }
}

#[test]
fn terminal_replays_bytes_accepts_input_resizes_and_closes_once() {
    let out = Arc::new(Output::default());
    let ctx = Arc::as_ptr(&out).cast_mut().cast::<c_void>();
    // SAFETY: Output stays valid until all detach/close operations complete.
    unsafe {
        assert!(!rs_term_attach(99, ctx, Some(output)));
    }
    rs_term_detach(99);
    rs_term_resize(99, 100, 30);
    let c = support::test_core();
    install_core(c.clone());
    let (term, mut input) = TermTap::new();
    let entry = c.state::<SshTable>().insert("test", term);
    entry.term.push_output(b"a");
    // SAFETY: The output context is held throughout the attachment.
    unsafe {
        assert!(!rs_term_attach(99, ctx, Some(output)));
        assert!(!rs_term_attach(entry.id, ctx, None));
        assert!(rs_term_attach(entry.id, ctx, Some(output)));
    }
    assert_eq!(*out.chunks.lock().unwrap(), [Some(b"a".to_vec())]);
    assert!(out
        .threads
        .lock()
        .unwrap()
        .iter()
        .all(|id| *id != thread::current().id()));
    // SAFETY: Valid input bytes are borrowed for the call; NULL/zero are ignored.
    unsafe {
        rs_term_input(entry.id, b"ls\n".as_ptr(), 3);
        rs_term_input(entry.id, ptr::null(), 3);
        rs_term_input(entry.id, b"x".as_ptr(), 0);
        rs_term_input(99, b"x".as_ptr(), 1);
    }
    rs_term_resize(entry.id, 100, 30);
    rs_term_resize(entry.id, 0, 30);
    assert_eq!(
        input.try_recv().unwrap(),
        TermInput::Bytes(b"ls\n".to_vec())
    );
    assert_eq!(
        input.try_recv().unwrap(),
        TermInput::Resize {
            cols: 100,
            rows: 30
        }
    );
    assert!(input.try_recv().is_err());
    rs_term_detach(entry.id);
    entry.term.push_output(b"buffered");
    assert_eq!(out.chunks.lock().unwrap().len(), 1);
    // SAFETY: Same live context; buffer replay must occur on a worker.
    unsafe {
        assert!(rs_term_attach(entry.id, ctx, Some(output)));
    }
    let worker_entry = entry.clone();
    c.block_on(c.handle().spawn(async move {
        worker_entry.term.push_output(b"live");
        worker_entry.term.close();
        worker_entry.term.close();
        worker_entry.term.push_output(b"ignored");
    }))
    .unwrap();
    assert_eq!(
        *out.chunks.lock().unwrap(),
        [
            Some(b"a".to_vec()),
            Some(b"buffered".to_vec()),
            Some(b"live".to_vec()),
            None
        ]
    );
    assert!(out
        .threads
        .lock()
        .unwrap()
        .iter()
        .all(|id| *id != thread::current().id()));
    // SAFETY: Closed/missing IDs must be safe with valid or NULL data.
    unsafe {
        assert!(!rs_term_attach(entry.id, ctx, Some(output)));
        rs_term_input(entry.id, b"x".as_ptr(), 1);
    }
    assert!(input.try_recv().is_err());
    rs_term_detach(entry.id);
    rs_term_detach(99);
    rs_term_resize(99, 1, 1);
    take_core();
    c.shutdown();
}
