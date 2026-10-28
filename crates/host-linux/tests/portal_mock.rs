#![cfg(target_os = "linux")]

#[path = "support/mock_portal.rs"]
mod mock_portal;

use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    io::Write,
    os::{fd::OwnedFd as StdOwnedFd, unix::fs::FileTypeExt, unix::net::UnixStream},
    pin::Pin,
    time::Duration,
};

use mock_portal::{MockConfig, MockPortal, MockStream, MockVal, Script};
use zbus::{
    export::futures_core::Stream, message::Type as MessageType, proxy::SignalStream, Connection,
    MatchRule, Message, MessageStream, OwnedFd, OwnedObjectPath, OwnedVariant, Proxy, Str,
    Structure, Variant,
};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const RD: &str = "org.freedesktop.portal.RemoteDesktop";
const SC: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
type Options = HashMap<String, OwnedVariant>;

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("portal mock test timed out")
}

async fn next_signal(stream: &mut SignalStream<'_>) -> Message {
    poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx))
        .await
        .expect("portal signal stream ended")
}

async fn proxy<'a>(conn: &Connection, path: &'a str, iface: &'a str) -> Proxy<'a> {
    Proxy::new(conn, PORTAL, path, iface)
        .await
        .expect("create portal proxy")
}

fn sender(conn: &Connection) -> String {
    conn.unique_name()
        .expect("client has bus unique name")
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_")
}

fn request_path(conn: &Connection, token: &str) -> String {
    format!("{DESKTOP}/request/{}/{token}", sender(conn))
}

fn text(value: &str) -> OwnedVariant {
    Str::from(value).into()
}

fn options(token: &str) -> Options {
    HashMap::from([("handle_token".into(), text(token))])
}

async fn call_request(
    conn: &Connection,
    iface: &str,
    method: &str,
    session: Option<&OwnedObjectPath>,
    options: &Options,
) -> OwnedObjectPath {
    let proxy = proxy(conn, DESKTOP, iface).await;
    match (method, session) {
        ("CreateSession", None) => proxy.call(method, &(options,)).await,
        ("Start", Some(session)) => proxy.call(method, &(session, "", options)).await,
        (_, Some(session)) => proxy.call(method, &(session, options)).await,
        _ => panic!("invalid test request"),
    }
    .expect("call portal request method")
}

async fn request(
    conn: &Connection,
    iface: &str,
    method: &str,
    session: Option<&OwnedObjectPath>,
    token: &str,
    mut extra: Options,
) -> (u32, Options) {
    extra.extend(options(token));
    let expected = request_path(conn, token);
    let request = proxy(conn, &expected, REQUEST).await;
    let mut signals = request.receive_signal("Response").await.unwrap();
    let handle = call_request(conn, iface, method, session, &extra).await;
    assert_eq!(handle.as_str(), expected);
    let signal = next_signal(&mut signals).await;
    assert_eq!(signal.header().path().unwrap().as_str(), expected);
    signal.body().deserialize().unwrap()
}

async fn create_session(conn: &Connection, iface: &str) -> OwnedObjectPath {
    let (code, results) = request(
        conn,
        iface,
        "CreateSession",
        None,
        "create",
        HashMap::from([("session_handle_token".into(), text("s"))]),
    )
    .await;
    assert_eq!(code, 0);
    let handle = <&str>::try_from(&results["session_handle"]).unwrap();
    OwnedObjectPath::try_from(handle.to_owned()).unwrap()
}

#[tokio::test]
async fn properties() {
    within(async {
        let gnome = MockPortal::start(MockConfig::gnome()).await;
        let client = gnome.client().await;
        let sc = proxy(&client, DESKTOP, SC).await;
        assert_eq!(sc.get_property::<u32>("version").await.unwrap(), 5);
        assert_eq!(
            sc.get_property::<u32>("AvailableSourceTypes")
                .await
                .unwrap(),
            7
        );
        assert_eq!(
            sc.get_property::<u32>("AvailableCursorModes")
                .await
                .unwrap(),
            7
        );
        let rd = proxy(&client, DESKTOP, RD).await;
        assert_eq!(rd.get_property::<u32>("version").await.unwrap(), 2);
        assert_eq!(
            rd.get_property::<u32>("AvailableDeviceTypes")
                .await
                .unwrap(),
            7
        );

        let wlr = MockPortal::start(MockConfig::wlr()).await;
        let client = wlr.client().await;
        let sc = proxy(&client, DESKTOP, SC).await;
        assert_eq!(sc.get_property::<u32>("version").await.unwrap(), 4);
        assert_eq!(
            sc.get_property::<u32>("AvailableSourceTypes")
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            sc.get_property::<u32>("AvailableCursorModes")
                .await
                .unwrap(),
            3
        );
        assert!(proxy(&client, DESKTOP, RD)
            .await
            .get_property::<u32>("AvailableDeviceTypes")
            .await
            .is_err());
    })
    .await;
}

#[tokio::test]
async fn create_session_response() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let client = mock.client().await;
        let (code, results) = request(
            &client,
            RD,
            "CreateSession",
            None,
            "t0",
            HashMap::from([("session_handle_token".into(), text("s"))]),
        )
        .await;
        assert_eq!(code, 0);
        assert!(matches!(&*results["session_handle"], Variant::Str(_)));
        assert_eq!(
            <&str>::try_from(&results["session_handle"]).unwrap(),
            format!("{DESKTOP}/session/{}/s", sender(&client))
        );
        let calls = mock.calls_to("CreateSession");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].iface, RD);
        assert_eq!(calls[0].path, DESKTOP);
        assert_eq!(calls[0].session, None);
        assert_eq!(
            calls[0].options,
            [
                ("handle_token".into(), MockVal::Str("t0".into())),
                ("session_handle_token".into(), MockVal::Str("s".into())),
            ]
        );
    })
    .await;
}

#[tokio::test]
async fn scripted_cancel_and_fail() {
    within(async {
        let mut cfg = MockConfig::gnome();
        cfg.scripts = vec![
            ("SelectDevices", Script::Cancel),
            ("Start", Script::Fail(2)),
        ];
        let mock = MockPortal::start(cfg).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        let (code, results) = request(
            &client,
            RD,
            "SelectDevices",
            Some(&session),
            "cancel",
            Options::new(),
        )
        .await;
        assert_eq!(code, 1);
        assert!(results.is_empty());
        let (code, results) =
            request(&client, RD, "Start", Some(&session), "fail", Options::new()).await;
        assert_eq!(code, 2);
        assert!(results.is_empty());

        let mut cfg = MockConfig::gnome();
        cfg.scripts = vec![("CreateSession", Script::NoSessionHandle)];
        let mock = MockPortal::start(cfg).await;
        let client = mock.client().await;
        let (code, results) = request(
            &client,
            RD,
            "CreateSession",
            None,
            "missing",
            HashMap::from([("session_handle_token".into(), text("missing"))]),
        )
        .await;
        assert_eq!(code, 0);
        assert!(!results.contains_key("session_handle"));
    })
    .await;
}

fn streams(results: &mut Options) -> Vec<(u32, Options)> {
    assert_eq!(
        results["streams"].value_signature().to_string(),
        "a(ua{sv})"
    );
    let streams: Vec<Structure<'_>> = results.remove("streams").unwrap().try_into().unwrap();
    streams
        .into_iter()
        .map(|stream| <(u32, Options)>::try_from(stream).unwrap())
        .collect()
}

fn pair(value: OwnedVariant) -> (i32, i32) {
    Structure::try_from(value).unwrap().try_into().unwrap()
}

#[tokio::test]
async fn start_results() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        let selected = request(
            &client,
            RD,
            "SelectDevices",
            Some(&session),
            "devices",
            HashMap::from([("types".into(), 3_u32.into())]),
        )
        .await;
        assert_eq!(selected.0, 0);
        let selected = request(
            &client,
            SC,
            "SelectSources",
            Some(&session),
            "sources",
            HashMap::from([
                ("types".into(), 1_u32.into()),
                ("multiple".into(), true.into()),
                ("cursor_mode".into(), 4_u32.into()),
            ]),
        )
        .await;
        assert_eq!(selected.0, 0);
        let (code, mut results) = request(
            &client,
            RD,
            "Start",
            Some(&session),
            "start",
            Options::new(),
        )
        .await;
        assert_eq!(code, 0);
        assert_eq!(u32::try_from(&results["devices"]).unwrap(), 3);
        assert_eq!(
            <&str>::try_from(&results["restore_token"]).unwrap(),
            "tok-1"
        );
        let mut streams = streams(&mut results);
        assert_eq!(streams.len(), 1);
        let (node_id, properties) = streams.pop().unwrap();
        assert_eq!(node_id, 42);
        assert_eq!(pair(properties["position"].clone()), (0, 0));
        assert_eq!(pair(properties["size"].clone()), (1920, 1080));
        assert_eq!(u32::try_from(&properties["source_type"]).unwrap(), 1);
        let calls = mock.calls();
        assert_eq!(
            calls
                .iter()
                .map(|call| call.member.as_str())
                .collect::<Vec<_>>(),
            ["CreateSession", "SelectDevices", "SelectSources", "Start"]
        );
        assert_eq!(calls[2].iface, SC);
        assert_eq!(calls[3].iface, RD);
        assert_eq!(calls[3].session.as_deref(), Some(session.as_str()));
        assert_eq!(calls[3].args, [MockVal::Str(String::new())]);
    })
    .await;
}

#[tokio::test]
async fn other_path() {
    within(async {
        let mut cfg = MockConfig::gnome();
        cfg.scripts = vec![("SelectSources", Script::OtherPath)];
        let mock = MockPortal::start(cfg).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        // Subscribe without a path before calling: the returned path is not predictable.
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface(REQUEST)
            .member("Response")
            .build()
            .unwrap();
        let mut signals = MessageStream::for_match_rule(rule, &client, None)
            .await
            .unwrap();
        let returned = call_request(
            &client,
            SC,
            "SelectSources",
            Some(&session),
            &options("expected"),
        )
        .await;
        assert_ne!(returned.as_str(), request_path(&client, "expected"));
        assert!(returned
            .as_str()
            .starts_with(&format!("{DESKTOP}/request/{}/other", sender(&client))));
        let response = poll_fn(|cx| Pin::new(&mut signals).poll_next(cx))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response.header().path().unwrap().as_str(),
            returned.as_str()
        );
        let (code, results): (u32, Options) = response.body().deserialize().unwrap();
        assert_eq!(code, 0);
        assert!(results.is_empty());
    })
    .await;
}

#[tokio::test]
async fn never_and_request_close() {
    within(async {
        let mut cfg = MockConfig::gnome();
        cfg.scripts = vec![("Start", Script::Never)];
        let mock = MockPortal::start(cfg).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        let path = request_path(&client, "never");
        let request = proxy(&client, &path, REQUEST).await;
        let mut signals = request.receive_signal("Response").await.unwrap();
        assert_eq!(
            call_request(&client, RD, "Start", Some(&session), &options("never"))
                .await
                .as_str(),
            path
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(1), next_signal(&mut signals))
                .await
                .is_err()
        );
        request.call::<_, _, ()>("Close", &()).await.unwrap();
        let closed = mock.calls_to("Close");
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].iface, REQUEST);
        assert_eq!(closed[0].path, path);
        assert!(request.call::<_, _, ()>("Close", &()).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn pipewire_fd() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        assert_eq!(mock.pipewire_fds(), 0);
        let sc = proxy(&client, DESKTOP, SC).await;
        let fd: OwnedFd = sc
            .call("OpenPipeWireRemote", &(&session, Options::new()))
            .await
            .unwrap();
        let mut socket = UnixStream::from(StdOwnedFd::from(fd));
        socket.set_nonblocking(true).unwrap();
        assert_eq!(socket.write(&[42]).unwrap(), 1);
        assert_eq!(mock.pipewire_fds(), 1);
        let calls = mock.calls_to("OpenPipeWireRemote");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].iface, SC);
        assert_eq!(calls[0].session.as_deref(), Some(session.as_str()));
    })
    .await;
}

#[tokio::test]
async fn notify_recorded() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        let rd = proxy(&client, DESKTOP, RD).await;
        rd.call::<_, _, ()>(
            "NotifyPointerMotionAbsolute",
            &(&session, Options::new(), 42_u32, 10.5_f64, 20.0_f64),
        )
        .await
        .unwrap();
        let calls = mock.calls_to("NotifyPointerMotionAbsolute");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].iface, RD);
        assert_eq!(calls[0].path, DESKTOP);
        assert_eq!(calls[0].session.as_deref(), Some(session.as_str()));
        assert_eq!(
            calls[0].args,
            [MockVal::U32(42), MockVal::F64(10.5), MockVal::F64(20.0)]
        );
        let unknown = OwnedObjectPath::try_from("/unknown").unwrap();
        let error = rd
            .call::<_, _, ()>(
                "NotifyPointerMotionAbsolute",
                &(unknown, Options::new(), 42_u32, 10.5_f64, 20.0_f64),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            zbus::Error::FDO(error) if matches!(*error, zbus::fdo::Error::InvalidArgs(_))
        ));
        assert_eq!(mock.calls_to("NotifyPointerMotionAbsolute").len(), 2);
    })
    .await;
}

#[tokio::test]
async fn close_session_emits_closed() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        let session_proxy = proxy(&client, session.as_str(), SESSION).await;
        let mut closed = session_proxy.receive_signal("Closed").await.unwrap();
        mock.close_session(session.as_str()).await;
        let (options,): (Options,) = next_signal(&mut closed).await.body().deserialize().unwrap();
        assert!(options.is_empty());
        assert!(session_proxy.call::<_, _, ()>("Close", &()).await.is_err());
        assert!(proxy(&client, DESKTOP, RD)
            .await
            .call::<_, _, ()>(
                "NotifyPointerMotion",
                &(session, Options::new(), 0_f64, 0_f64)
            )
            .await
            .is_err());
    })
    .await;
}

#[tokio::test]
async fn no_name_registered() {
    within(async {
        let mut cfg = MockConfig::gnome();
        cfg.register_name = false;
        let mock = MockPortal::start(cfg).await;
        let client = mock.client().await;
        let error = proxy(&client, DESKTOP, SC)
            .await
            .get_property::<u32>("version")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            zbus::Error::FDO(error)
                if matches!(
                    *error,
                    zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)
                )
        ));
        assert!(mock.calls().is_empty());
    })
    .await;
}

#[tokio::test]
async fn parallel_buses() {
    within(async {
        let (first, second) = tokio::join!(
            MockPortal::start(MockConfig::gnome()),
            MockPortal::start(MockConfig::wlr()),
        );
        assert_ne!(first.address(), second.address());
        let (client1, client2) = tokio::join!(first.client(), second.client());
        create_session(&client1, RD).await;
        assert_eq!(first.calls_to("CreateSession").len(), 1);
        assert!(second.calls().is_empty());
        create_session(&client2, SC).await;
        assert_eq!(second.calls_to("CreateSession")[0].iface, SC);
        assert_eq!(first.calls_to("CreateSession")[0].iface, RD);
        assert_eq!(first.calls().len(), 1);
        assert_eq!(second.calls().len(), 1);
    })
    .await;
}

#[tokio::test]
async fn screencast_optional_results() {
    within(async {
        let mut cfg = MockConfig::wlr();
        cfg.restore_token_out = None;
        cfg.streams.push(MockStream {
            node_id: 44,
            position: Some((-1920, -100)),
            size: None,
            source_type: None,
        });
        let mock = MockPortal::start(cfg).await;
        let client = mock.client().await;
        let session = create_session(&client, SC).await;
        assert_eq!(
            request(
                &client,
                SC,
                "SelectSources",
                Some(&session),
                "select",
                Options::new(),
            )
            .await
            .0,
            0
        );
        let (code, mut results) = request(
            &client,
            SC,
            "Start",
            Some(&session),
            "start",
            Options::new(),
        )
        .await;
        assert_eq!(code, 0);
        assert!(!results.contains_key("devices"));
        assert!(!results.contains_key("restore_token"));
        let streams = streams(&mut results);
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].0, 43);
        assert!(!streams[0].1.contains_key("position"));
        assert_eq!(streams[1].0, 44);
        assert_eq!(pair(streams[1].1["position"].clone()), (-1920, -100));
        assert!(!streams[1].1.contains_key("size"));
        assert!(!streams[1].1.contains_key("source_type"));
    })
    .await;
}

#[tokio::test]
async fn all_notify_wire_signatures_and_option_types() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let client = mock.client().await;
        let session = create_session(&client, RD).await;
        let rd = proxy(&client, DESKTOP, RD).await;
        let options: Options = HashMap::from([
            ("string".into(), text("example")),
            ("u32".into(), 7_u32.into()),
            ("i32".into(), (-3_i32).into()),
            ("bool".into(), true.into()),
            ("f64".into(), 1.5_f64.into()),
            ("path".into(), zbus::ObjectPath::from(&session).into()),
            (
                "tuple".into(),
                OwnedVariant::try_from(Variant::new((1_i32, 2_i32))).unwrap(),
            ),
        ]);
        rd.call::<_, _, ()>("NotifyPointerMotion", &(&session, &options, -1_f64, 2_f64))
            .await
            .unwrap();
        rd.call::<_, _, ()>(
            "NotifyPointerButton",
            &(&session, Options::new(), 272_i32, 1_u32),
        )
        .await
        .unwrap();
        rd.call::<_, _, ()>(
            "NotifyPointerAxis",
            &(&session, Options::new(), 1.25_f64, -2.5_f64),
        )
        .await
        .unwrap();
        rd.call::<_, _, ()>(
            "NotifyPointerAxisDiscrete",
            &(&session, Options::new(), 0_u32, -2_i32),
        )
        .await
        .unwrap();
        rd.call::<_, _, ()>(
            "NotifyKeyboardKeycode",
            &(&session, Options::new(), 30_i32, 1_u32),
        )
        .await
        .unwrap();
        rd.call::<_, _, ()>(
            "NotifyKeyboardKeysym",
            &(&session, Options::new(), 97_i32, 0_u32),
        )
        .await
        .unwrap();
        let calls = mock.calls();
        assert_eq!(calls.len(), 7);
        assert_eq!(calls[1].args, [MockVal::F64(-1.0), MockVal::F64(2.0)]);
        assert_eq!(calls[2].args, [MockVal::I32(272), MockVal::U32(1)]);
        assert_eq!(calls[3].args, [MockVal::F64(1.25), MockVal::F64(-2.5)]);
        assert_eq!(calls[4].args, [MockVal::U32(0), MockVal::I32(-2)]);
        assert_eq!(calls[5].args, [MockVal::I32(30), MockVal::U32(1)]);
        assert_eq!(calls[6].args, [MockVal::I32(97), MockVal::U32(0)]);
        let options = &calls[1].options;
        assert_eq!(options[0], ("bool".into(), MockVal::Bool(true)));
        assert_eq!(options[1], ("f64".into(), MockVal::F64(1.5)));
        assert_eq!(options[2], ("i32".into(), MockVal::I32(-3)));
        assert_eq!(
            options[3],
            ("path".into(), MockVal::Path(session.to_string()))
        );
        assert_eq!(
            options[4],
            ("string".into(), MockVal::Str("example".into()))
        );
        assert!(matches!(&options[5], (key, MockVal::Other(_)) if key == "tuple"));
        assert_eq!(options[6], ("u32".into(), MockVal::U32(7)));
    })
    .await;
}

#[tokio::test]
async fn session_close_method_and_multiple_clients() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let client1 = mock.client().await;
        // address() also works with an ordinary zbus bus connection.
        let client2 = zbus::connection::Builder::address(mock.address())
            .build()
            .await
            .unwrap();
        assert_ne!(client1.unique_name(), client2.unique_name());
        let (session1, session2) =
            tokio::join!(create_session(&client1, RD), create_session(&client2, RD),);
        assert_ne!(session1, session2);
        // A second client sees the same exported objects and broadcast signals.
        let observer = proxy(&client2, session1.as_str(), SESSION).await;
        let mut closed = observer.receive_signal("Closed").await.unwrap();
        let session = proxy(&client1, session1.as_str(), SESSION).await;
        session.call::<_, _, ()>("Close", &()).await.unwrap();
        let signal = next_signal(&mut closed).await;
        assert_eq!(signal.header().path().unwrap().as_str(), session1.as_str());
        assert!(session.call::<_, _, ()>("Close", &()).await.is_err());
        let calls = mock.calls_to("Close");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].iface, SESSION);
        assert_eq!(calls[0].path, session1.as_str());
        assert_eq!(calls[0].session.as_deref(), Some(session1.as_str()));
        proxy(&client2, DESKTOP, RD)
            .await
            .call::<_, _, ()>(
                "NotifyPointerMotion",
                &(&session2, Options::new(), 0_f64, 0_f64),
            )
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
async fn drop_removes_private_socket() {
    within(async {
        let mock = MockPortal::start(MockConfig::gnome()).await;
        let path = std::path::PathBuf::from(mock.address().strip_prefix("unix:path=").unwrap());
        let client = mock.client().await;
        assert!(path.metadata().unwrap().file_type().is_socket());
        drop(mock);
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
        client.closed().await;
    })
    .await;
}
