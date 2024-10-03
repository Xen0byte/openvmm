// Copyright (C) Microsoft Corporation. All rights reserved.

//! A generic protobuf service for inspect.

// Crates used by generated code. Reference them explicitly to ensure that
// automated tools do not remove them.
use mesh::payload::Downcast;
use mesh_ttrpc as _;
use prost as _;

include!(concat!(env!("OUT_DIR"), "/inspect.rs"));

/// Equivalent to [`InspectResponse`], but using [`inspect::Node`].
/// These have equivalent encodings.
#[derive(Debug, Clone, mesh::MeshPayload)]
pub struct InspectResponse2 {
    #[mesh(1)]
    pub result: inspect::Node,
}

impl Downcast<InspectResponse> for InspectResponse2 {}
impl Downcast<InspectResponse2> for InspectResponse {}

/// Equivalent to [`InspectResponse`], but using [`inspect::Value`].
/// These have equivalent encodings.
#[derive(Debug, Clone, mesh::MeshPayload)]
pub struct UpdateResponse2 {
    #[mesh(1)]
    pub new_value: inspect::Value,
}

impl Downcast<UpdateResponse> for UpdateResponse2 {}
impl Downcast<UpdateResponse2> for UpdateResponse {}

#[cfg(test)]
mod tests {
    use crate::InspectResponse;
    use crate::InspectResponse2;
    use inspect::Entry;
    use inspect::Error;
    use inspect::Node;
    use inspect::SensitivityLevel;
    use inspect::Value;
    use inspect::ValueKind;
    use mesh::Message;

    #[test]
    fn test() {
        let response2 = InspectResponse2 {
            result: Node::Dir(vec![
                Entry {
                    name: "a".to_string(),
                    node: Node::Unevaluated,
                    sensitivity: Some(SensitivityLevel::Unspecified),
                },
                Entry {
                    name: "b".to_string(),
                    node: Node::Failed(Error::Update("foo".into())),
                    sensitivity: Some(SensitivityLevel::Safe),
                },
                Entry {
                    name: "c".to_string(),
                    node: Node::Value(Value::new(ValueKind::Signed(-1))),
                    sensitivity: Some(SensitivityLevel::Sensitive),
                },
                Entry {
                    name: "d".to_string(),
                    node: Node::Value(Value::new(ValueKind::Unsigned(2))),
                    sensitivity: Some(SensitivityLevel::Safe),
                },
                Entry {
                    name: "e".to_string(),
                    node: Node::Value(Value::new(ValueKind::Bool(true))),
                    sensitivity: Some(SensitivityLevel::Sensitive),
                },
                Entry {
                    name: "f".to_string(),
                    node: Node::Value(Value::new(ValueKind::String("foo".to_string()))),
                    sensitivity: Some(SensitivityLevel::Unspecified),
                },
                Entry {
                    name: "g".to_string(),
                    node: Node::Value(Value::new(ValueKind::Bytes(b"abc".to_vec()))),
                    sensitivity: None,
                },
            ]),
        };

        let response = Message::new(response2.clone())
            .parse::<InspectResponse>()
            .unwrap();

        assert_eq!(
            Message::new(response)
                .parse::<InspectResponse2>()
                .unwrap()
                .result,
            response2.result
        );
    }
}