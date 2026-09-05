#![cfg(feature = "structured")]
use agent_bridge::structured::{JsonContract, JsonRejection};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Count {
    count: u32,
}

fn contract() -> JsonContract<Count> {
    JsonContract::new(
        "count",
        "v1",
        "Return a JSON object with integer count.",
        128,
    )
    .unwrap()
}

#[test]
fn validates_typed_shape_and_application_rules_without_repair() {
    let contract = contract().with_validation(|result| {
        if result.count == 3 {
            Ok(())
        } else {
            Err("expected three items".into())
        }
    });
    assert_eq!(
        contract.validate(" {\"count\":3}\n").unwrap(),
        Count { count: 3 }
    );
    assert!(matches!(
        contract.validate("{\"count\":4}"),
        Err(JsonRejection::InvalidValue(_))
    ));
    for text in [
        "{\"count\":\"3\"}",
        "{\"count\":3,\"extra\":true}",
        "{}",
        "{\"count\":3,\"count\":4}",
    ] {
        assert!(matches!(
            contract.validate(text),
            Err(JsonRejection::InvalidShape(_))
        ));
    }
    for text in [
        "```json\n{\"count\":3}\n```",
        "{\"count\":3} trailing",
        "{\"count\":3}{\"count\":4}",
        "{\"count\":",
    ] {
        assert!(matches!(
            contract.validate(text),
            Err(JsonRejection::InvalidJson(_))
        ));
    }
}

#[test]
fn bounds_output_before_parsing_and_rejects_invalid_contracts() {
    assert!(JsonContract::<Count>::new("", "v1", "JSON", 128).is_err());
    assert!(JsonContract::<Count>::new("count", "", "JSON", 128).is_err());
    assert!(JsonContract::<Count>::new("count", "v1", "", 128).is_err());
    assert!(JsonContract::<Count>::new("count", "v1", "JSON", 0).is_err());
    assert_eq!(contract().validate(" "), Err(JsonRejection::MissingOutput));
    assert_eq!(
        contract().validate(&"x".repeat(129)),
        Err(JsonRejection::OutputTooLarge)
    );
}
