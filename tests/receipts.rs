#![cfg(feature = "receipts")]
use agent_bridge::records::{Payload, receipts::*};
use serde_json::{Value, json};

fn payload(name: &str, data: Value) -> Payload {
    Payload::Extension {
        namespace: "agent_bridge".into(),
        name: name.into(),
        data,
    }
}
fn input(version: u32) -> Value {
    let mut value = json!({"version":version,"state":"prepared","encoding":"agent_bridge.text_context.v1","context_mode":"append_to_native","wire_text":"🌍","wire_bytes":4,"omissions":[]});
    if version == 2 {
        value["encoding"] = json!("agent_bridge.media_context.v1");
        value["wire_bytes"] = json!(100);
        value["images"] = json!([{"reference":{"id":"image","revision":"v1"},"media_type":"image/png","sha256":"0".repeat(64),"bytes":1,"prompt_block":1}]);
        value["resource_retention"] = json!("supplied_resource_store");
    }
    if version == 3 {
        value["instruction_authority"] = Value::Null;
        value["requested_context"] = json!({"records":[],"resources":[],"instructions":[]});
    }
    if version == 4 {
        value["skills"] = json!([{"resource":{"id":"skill","revision":"v1"},"planned_delivery":"omitted","local_availability":"not_checked","native_availability":"unknown","native_activation":"not_observed","reason":"host chose omission"}]);
    }
    value
}

#[test]
fn shared_receipt_vectors_distinguish_supported_unknown_and_malformed() {
    let vectors: Vec<Value> =
        serde_json::from_str(include_str!("../verification/receipts.json")).unwrap();
    for vector in vectors {
        let source = Payload::Extension {
            namespace: vector["namespace"]
                .as_str()
                .unwrap_or("agent_bridge")
                .into(),
            name: vector["name"].as_str().unwrap().into(),
            data: vector["data"].clone(),
        };
        let original = source.clone();
        let result = read(&source);
        let actual = match result {
            Err(_) => "invalid".into(),
            Ok(None) => "none".into(),
            Ok(Some(receipt)) => serde_json::to_value(receipt).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned(),
        };
        assert_eq!(actual, vector["expected"].as_str().unwrap());
        assert_eq!(source, original);
    }
}

#[test]
fn input_versions_keep_preparation_delivery_and_native_skill_evidence_separate() {
    for version in 1..=4 {
        let source = payload("input_receipt", input(version));
        let Receipt::Input(value) = read(&source).unwrap().unwrap() else {
            panic!()
        };
        let InputStage::Prepared(prepared) = value.stage else {
            panic!()
        };
        assert_eq!(prepared.wire_text, "🌍");
        if version == 4 {
            assert_eq!(
                prepared.skills.unwrap()[0].native_activation,
                "not_observed"
            );
        }
        for state in ["dispatch_attempted", "unknown", "response_received"] {
            let source = payload(
                "input_receipt",
                json!({"version":version,"state":state,"stop_reason":"end_turn"}),
            );
            let Receipt::Input(value) = read(&source).unwrap().unwrap() else {
                panic!()
            };
            assert!(!matches!(value.stage, InputStage::Prepared(_)));
        }
    }
    let mut bad = input(4);
    bad["skills"][0]["native_activation"] = json!("active");
    assert!(read(&payload("input_receipt", bad)).is_err());
    let mut bad = input(2);
    bad["images"][0]["sha256"] = json!("invalid");
    assert!(read(&payload("input_receipt", bad)).is_err());
    let mut bad = input(3);
    bad.as_object_mut().unwrap().remove("requested_context");
    assert!(read(&payload("input_receipt", bad)).is_err());
}

#[test]
fn restoration_reports_do_not_claim_delivered_context_or_native_state_transfer() {
    for version in 1..=3 {
        let mut value = json!({"version":version,"strategy":"portable_selection","native_context":"new_session","session_id":"s","slot_id":"slot","selected_records":[{"id":"r","revision":9007199254740993u64}],"selected_resources":[],"selected_instructions":[],"not_transferred":["provider_hidden_state"],"delivery":"pending_first_run"});
        if version >= 2 {
            value["context_policy"] = json!({"omissions":[],"instruction_authority":null,"requested_context":{"records":[],"resources":[],"instructions":[]},"skills":[]});
        }
        let Receipt::Restoration(report) = read(&payload("restoration", value.clone()))
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        let RestorationStrategy::PortableSelection {
            selected_records,
            delivery,
            ..
        } = report.strategy
        else {
            panic!()
        };
        assert_eq!(selected_records[0].revision, 9007199254740993);
        assert_eq!(delivery, "pending_first_run");
        value["native_context"] = json!("transferred");
        assert!(read(&payload("restoration", value)).is_err());
    }
}

#[test]
fn known_invalid_versions_and_missing_evidence_do_not_turn_into_defaults() {
    for version in [Value::Null, json!(0), json!(-1), json!(1.5), json!("1")] {
        assert!(
            read(&payload(
                "input_receipt",
                json!({"version":version,"state":"unknown"})
            ))
            .is_err()
        );
    }
    assert!(read(&payload("configuration_report", json!({}))).is_err());
    assert!(matches!(
        read(&payload("configuration_report", json!({"version":1}))).unwrap(),
        Some(Receipt::Unsupported { .. })
    ));
    let mut value = input(1);
    value["extension_added_later"] = json!({"keep":true});
    assert!(matches!(
        read(&payload("input_receipt", value)).unwrap(),
        Some(Receipt::Input(_))
    ));
}

#[test]
fn invocation_receipts_keep_returned_results_separate_from_uncertain_effects() {
    let base = json!({"version":1,"invocation_id":"call","binding_id":"binding","scope":{"session":"s","slot":"slot"},"tool":{"name":"lookup","revision":"v1"},"issuer":"host","subject":"assistant"});
    for (state, fields) in [
        ("dispatch_attempted", json!({"input":{"key":"project"}})),
        (
            "returned",
            json!({"outcome":{"kind":"success","value":{"count":7}}}),
        ),
        (
            "unknown",
            json!({"reason":"cancelled while application was running"}),
        ),
    ] {
        let mut data = base.clone();
        data["state"] = json!(state);
        data.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert!(matches!(
            read(&payload("tool_invocation", data)).unwrap(),
            Some(Receipt::ToolInvocation(_))
        ));
    }
    let mut invalid = base.clone();
    invalid["state"] = json!("returned");
    assert!(read(&payload("tool_invocation", invalid)).is_err());
    let mut future = base;
    future["version"] = json!(2);
    assert!(matches!(
        read(&payload("tool_invocation", future)).unwrap(),
        Some(Receipt::Unsupported { .. })
    ));
}
