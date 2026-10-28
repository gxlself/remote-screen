# RSP/1 fuzz targets

This is an independent Cargo workspace (`fuzz`) so fuzz builds do not become
workspace members of the main project. It targets the decoders that are already
merged. The video-header and reassembly targets cover the video datagram and
receiver state machine; the file-header and file-name targets cover the
file-format modules.

| Target | Decoder exercised | Invariant |
|---|---|---|
| `input_event` | `rsp_core::hid::InputMsg::decode` | no panic; accepted messages re-encode and decode to the same value |
| `pointer` | `PointerDatagram::decode`, `PointerFilter` | no panic; an immediate duplicate sequence is rejected |
| `cursor` | `wire::FrameDecoder`, `pb::CursorMsg`, `ShapeCache` | no panic; cache length never exceeds 64 |
| `clipboard_header` | `clipboard::StreamReceiver` | no panic; oversized header/data lengths are rejected |
| `tcp_record` | `rsp_transport::tcp::TcpRecordDecoder` | no panic; every emitted payload respects the channel limit, and oversized headers are rejected |
| `pair_qr` | `rsp_core::qr::PairQr::parse` | no panic; parsed QR values generate a URL that parses to the same value |
| `pair_spake` | `AppPairing` and `HostPairing` | no panic; mismatched six-digit codes can never reach the verified state |
| `fec_recv` | `rsp_core::fec::recover` | no panic, index/length/parameter validation stays total, and the modeled receiver never keeps more than 8 frames in flight |
| `wire_frames` | `rsp_core::wire::FrameDecoder` | no panic; oversized frames are rejected |
| `envelope` | `rsp_core::control::EnvelopeDecoder`, `pb::Envelope::decode` | no panic; framing errors are sticky, oversized lengths fail before payloads arrive, and every accepted envelope re-encodes byte-for-byte |
| `trust_statement` | `rsp_core::trust::verify`, `apply_log` | no panic; approved/applied sequences are ordered and disjoint, approvals carry the fixed signer and certificate, and sequence gaps match the input |
| `file_header` | `rsp_core::file::FileStreamReceiver` | no panic; oversized lengths are rejected before payloads arrive, the header appears at most once, spans are contiguous and in bounds, and FIN agrees with the declared size |
| `file_name` | `rsp_core::file_name::{sanitize, candidate}` | no panic; sanitized names and collision candidates are safe, bounded components, and sanitizing is idempotent |
| `audio_datagram` | `rsp_core::audio::decode` | no panic; every accepted datagram re-encodes byte-for-byte |
| `video_header` | `rsp_core::video::VideoHeader::parse` | no panic; an accepted header re-encodes to the input's first `HEADER_LEN` bytes and the returned payload is the exact input suffix |
| `reassembly` | `Reassembler::with_fps`, `push`, `tick`, `set_fps`, `pause_deadlines` | no panic; after every operation the clock is monotonic, `slot_count() <= MAX_SLOTS`, `buffered_bytes() <= MAX_REASSEMBLY_BYTES`, and every emitted frame is no longer than the bytes available to the reassembler |
| `pairfile` | `rsp_core::pairfile::parse` | no panic; input over `PAIRFILE_MAX_BYTES` is always `TooLarge`; accepted values have version 1, nonzero expiry, 1–8 addresses and a ≤64-character name, and `to_uri()` round-trips every field |
| `file_ctl` | `pb::{FileList, FileRequest, FileQuery, FileStatus, FileCancel}` and the matching `validate_*` | no panic; accepted paths are non-empty where required, ≤`MAX_PATH_BYTES` and contain no NUL; accepted transfer IDs are 32 lowercase hex characters; resume offsets and SHA-256 lengths match the module rules |
| `pipe_frames` | `host_win::pipe_proto::FrameReader`, `encode_frame` | no panic; a declared length over `MAX_FRAME` fails before its payload; every accepted frame re-encodes and decodes to the same bytes; the reader stops after its first error |
| `bind_node` | `rsp_core::trust::check_bind_node` | no panic; `Ok` requires a valid signature under the fixed P-256 test key, kind `BindNode`, and the requested user |

`fec_recv` drives the receive-side `recover` entry point. The current `rsp-core`
API has no persistent FEC receiver type, so the target groups datagrams by
`(epoch, frame_id)` and bounds that model at eight slots, matching protocol §5.3.
The `reassembly` target exercises the real `Reassembler` and its eight-slot,
64 MiB limits, so it covers the receiver integration that this model omits.

`trust_statement` lives in `rsp-core` because `SignedStatement` decoding and
signature verification are implemented in `rsp_core::trust`; `host-core` only
persists the resulting trust actions. Journal sequences are local metadata;
only the statement, signer fingerprint and signature travel on the wire.

`pipe_frames` adds a dependency on `host-win` so it can drive
`host_win::pipe_proto::FrameReader` directly. Only the pure protocol module is
used; the Win32 modules remain gated and this target builds on Linux.

`file_ctl` deliberately asserts the rules implemented by
`rsp_core::file_ctl::validate_*`: the path check there is length/NUL/emptiness,
not `..` rejection. Root-escape prevention happens later in
`host-core::file_browse` after canonicalization, so a `..` segment alone is not
a `file_ctl` failure.

## Corpus

`fuzz/corpus/<target>/seed-*` contains small seeds; every file is at most
64 KiB. The `envelope` seeds come from the binary vectors under
`crates/rsp-core/tests/vectors/control/`, which implement the test-vector
requirements in `docs/dev/03-protocol.md` §12, plus boundary inputs written
directly in the control-stream format. `audio_datagram` uses the vectors under
`crates/rsp-core/tests/vectors/audio/`; `video_header` uses
`crates/rsp-core/tests/vectors/video/`; `reassembly` seeds encode operation
sequences around those video datagrams; and `bind_node` uses the BIND_NODE and
other trust vectors under `crates/rsp-core/tests/vectors/trust/`. The
`trust_statement` seeds are structured `Unstructured` inputs generated once from
the fixed signer and subject material. Only hand-written and copied seeds are
committed; generated corpora and crash artifacts are ignored.

## Run

```sh
just fuzz input_event 60
bash tools/fuzz/run-all.sh 60
```

`run-all.sh` runs every target sequentially with
`-max_total_time=<seconds>`, prints a summary with the final `exec/s` and `cov`,
and exits non-zero if any target crashes or is missing. It never installs
toolchain components; if `cargo +nightly fuzz --version` fails, it reports that
and exits with status 2.
