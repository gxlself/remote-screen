//! Map the negotiated path and transport to the rate controller's bounds.

use rsp_core::{pb, ratectl::Path};

pub fn path_of(kind: pb::PathKind, transport: pb::Transport) -> Path {
    if transport == pb::Transport::Tcp {
        return Path::Tcp;
    }
    match kind {
        pb::PathKind::Lan => Path::Lan,
        pb::PathKind::Intranet | pb::PathKind::Manual => Path::Intranet,
        pb::PathKind::TailnetDirect => Path::TailnetDirect,
        pb::PathKind::TailnetDerp | pb::PathKind::PathUnspecified => Path::Derp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_all_paths_and_prioritizes_tcp() {
        for (kind, expected) in [
            (pb::PathKind::Lan, Path::Lan),
            (pb::PathKind::Intranet, Path::Intranet),
            (pb::PathKind::Manual, Path::Intranet),
            (pb::PathKind::TailnetDirect, Path::TailnetDirect),
            (pb::PathKind::TailnetDerp, Path::Derp),
            (pb::PathKind::PathUnspecified, Path::Derp),
        ] {
            for transport in [pb::Transport::Quic, pb::Transport::Unspecified] {
                assert_eq!(path_of(kind, transport), expected);
            }
            assert_eq!(path_of(kind, pb::Transport::Tcp), Path::Tcp);
        }
    }
}
