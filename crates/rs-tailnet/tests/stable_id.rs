use rs_tailnet::TailnetStatus;

#[test]
fn self_stable_id_roundtrips_and_defaults() {
    let with_id: TailnetStatus =
        serde_json::from_str(r#"{"state":"running","self":{"stableId":"n1"}}"#)
            .expect("status with stable id");
    assert_eq!(with_id.self_node.as_ref().unwrap().stable_id, "n1");
    let encoded = serde_json::to_string(&with_id).expect("serialize status");
    let decoded: TailnetStatus = serde_json::from_str(&encoded).expect("deserialize status");
    assert_eq!(decoded, with_id);

    let without_id: TailnetStatus =
        serde_json::from_str(r#"{"state":"running","self":{}}"#).expect("status without id");
    assert_eq!(without_id.self_node.as_ref().unwrap().stable_id, "");
}

#[test]
fn go_status_fixture_carries_stable_id() {
    let status: TailnetStatus =
        serde_json::from_str(include_str!("../../../tailnet/testdata/status.json"))
            .expect("status fixture");
    assert_eq!(status.self_node.unwrap().stable_id, "nT3stSelf1");
}
