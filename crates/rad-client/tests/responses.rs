use rad_client::response::*;
use serde_json::json;
const SESSION: &str = "00000000000000000000000000000001";
fn good() -> serde_json::Value {
    json!({"v":1,"id":"2","session_id":SESSION,"status":"applied","data":{"changed":true,"revision":"10"},"error":null})
}
#[test]
fn response_is_accepted_only_for_the_exact_request_and_runtime_session() {
    let bytes = serde_json::to_vec(&good()).unwrap();
    let response = decode(&bytes, 2, Some(SESSION)).unwrap();
    assert_eq!(response.status, Status::Applied);
    assert_eq!(
        serde_json::to_value(response.data).unwrap(),
        json!({"changed":true,"revision":"10"})
    );
    assert!(decode(&bytes, 3, Some(SESSION)).is_err());
    assert!(decode(&bytes, 2, Some("00000000000000000000000000000002")).is_err());
}
#[test]
fn response_requires_all_fields_closed_codes_and_consistent_data_error_shapes() {
    for field in ["v", "id", "session_id", "status", "data", "error"] {
        let mut v = good();
        v.as_object_mut().unwrap().remove(field);
        assert!(
            decode(&serde_json::to_vec(&v).unwrap(), 2, None).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("v", json!(2)),
        ("id", json!("02")),
        ("session_id", json!("0")),
        ("session_id", json!("00000000000000000000000000000000")),
        ("status", json!("unknown")),
        ("data", json!(null)),
        ("extra", json!(true)),
    ] {
        let mut v = good();
        v[field] = value;
        assert!(
            decode(&serde_json::to_vec(&v).unwrap(), 2, None).is_err(),
            "{v}"
        );
    }
    let mut error = good();
    error["status"] = json!("error");
    error["data"] = json!(null);
    error["error"] = json!({"code":"storage_uncertain","message":"Storage result is uncertain","details":{},"outcome":"unknown"});
    assert_eq!(
        decode(&serde_json::to_vec(&error).unwrap(), 2, Some(SESSION))
            .unwrap()
            .error
            .unwrap()
            .outcome,
        Some(Outcome::Unknown)
    );
    for (field, value) in [
        ("code", json!("invented")),
        ("message", json!("x".repeat(257))),
        ("details", json!([])),
        ("outcome", json!("cancelled")),
    ] {
        let mut v = error.clone();
        v["error"][field] = value;
        assert!(decode(&serde_json::to_vec(&v).unwrap(), 2, None).is_err());
    }
    let mut v = error.clone();
    v["data"] = json!({});
    assert!(decode(&serde_json::to_vec(&v).unwrap(), 2, None).is_err());
    let mut v = good();
    v["error"] = error["error"].clone();
    assert!(decode(&serde_json::to_vec(&v).unwrap(), 2, None).is_err());
    for field in ["code", "message", "details", "outcome"] {
        let mut v = error.clone();
        v["error"].as_object_mut().unwrap().remove(field);
        assert!(decode(&serde_json::to_vec(&v).unwrap(), 2, None).is_err());
    }
}
#[test]
fn applied_and_admitted_receipts_do_not_claim_success_without_their_required_identity() {
    for data in [
        json!({}),
        json!({"changed":true}),
        json!({"changed":false,"revision":"01"}),
        json!({"changed":1,"revision":"0"}),
    ] {
        let mut v = good();
        v["data"] = data;
        assert!(decode(&serde_json::to_vec(&v).unwrap(), 2, None).is_err());
    }
    let mut v = good();
    v["status"] = json!("admitted");
    for data in [
        json!({}),
        json!({"execution_id":"bad"}),
        json!({"execution_id":format!("{SESSION}:0")}),
        json!({"execution_id":"00000000000000000000000000000002:1"}),
    ] {
        v["data"] = data;
        assert!(Response::decode_value(v.clone(), 2, None, rad_api::Operation::Run).is_err());
    }
    v["data"] = json!({"execution_id":format!("{SESSION}:1")});
    assert!(Response::decode_value(v, 2, None, rad_api::Operation::Run).is_ok());
    let mut v = good();
    v["data"] = json!({"changed":false,"revision":"0"});
    assert!(Response::decode_value(v, 2, None, rad_api::Operation::Edit).is_ok());
}

fn decode(bytes: &[u8], id: u64, session: Option<&str>) -> Result<Response, InvalidResponse> {
    Response::decode_value(
        rad_wire::parse_json(bytes).map_err(|_| InvalidResponse)?,
        id,
        session,
        rad_api::Operation::Edit,
    )
}
