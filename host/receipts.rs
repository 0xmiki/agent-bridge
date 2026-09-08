use agent_bridge::records::{Payload, receipts};
use serde_json::{Value, json};

pub fn view(payload: &Payload) -> Value {
    match receipts::read(payload) {
        Ok(None) => Value::Null,
        Ok(Some(receipt)) => {
            let mut value = serde_json::to_value(receipt).expect("receipt serialization");
            // Keep the exact request text once, in the original payload. The TS
            // reader exposes it from there without duplicating it on the wire.
            if let Some(data) = value["data"].as_object_mut() {
                data.remove("wire_text");
            }
            if value["kind"] == "tool_invocation" {
                let data = value["data"].as_object_mut().unwrap();
                // Arbitrary application JSON is retained once and must not have
                // its numbers converted into metadata counters.
                data.remove("input");
                data.remove("outcome");
            }
            decimal_numbers(&mut value);
            value
        }
        Err(error) => json!({"kind":"invalid","data":{"name":error.name,"reason":error.reason}}),
    }
}
fn decimal_numbers(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key != "version" {
                    decimal_numbers(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                decimal_numbers(item);
            }
        }
        Value::Number(number) => *value = Value::String(number.to_string()),
        _ => {}
    }
}
