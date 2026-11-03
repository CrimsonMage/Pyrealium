// SPDX-License-Identifier: AGPL-3.0-only
//! Admin7148 ExaminationRequest::StreamPack 00609660 and ExaminationProfile::StreamPack
//! 00608380. The shared codec uses the later Legions request union (one extra word).
use ac2_net::protocol::*;
use serde_json::{json, Value};

pub fn request(id: u64) -> ClientEvent {
    let words = [
        5,
        1,
        1,
        0x44000100,
        0,
        0xffff009a,
        0x40000002,
        0,
        id as u32,
        (id >> 32) as u32,
        0,
    ];
    ClientEvent::Interp {
        encrypted: false,
        function: 0x820a0007,
        arguments: words.into_iter().flat_map(u32::to_le_bytes).collect(),
    }
}

fn profile(bytes: &[u8]) -> Option<ExaminationProfile> {
    let decode = |data: &[u8]| match ScriptMessage::decode(0x820a0009, data)? {
        Ok(ScriptMessage::ExaminationProfile(p)) => Some(p),
        _ => None,
    };
    // Only adapt a complete, bounded native object-profile header; the pinned codec still
    // validates node types, references, strings, counts and trailing heap fields.
    if (48..=65536).contains(&bytes.len()) {
        let word = |n: usize| u32::from_le_bytes(bytes[n * 4..n * 4 + 4].try_into().unwrap());
        if word(0) == 5
            && word(1) == 1
            && ![0, u32::MAX].contains(&word(2))
            && word(3) == 0x44000100
            && word(4) == 0
            && word(5) == 0xffff0099
            && word(6) == 0x40000002
            && word(7) == 0
            && word(10) <= 1024
        {
            let mut adapted = bytes.to_vec();
            adapted.splice(40..40, 0u32.to_le_bytes());
            if let Some(p) = decode(&adapted) {
                return Some(p);
            }
        }
    }
    decode(bytes)
}

fn kind(v: &ExaminationValue) -> u32 {
    0x40000000
        | match v {
            ExaminationValue::Int(_) => 1,
            ExaminationValue::Float(_) => 2,
            ExaminationValue::Bool(_) => 3,
            ExaminationValue::Text(_) => 4,
            ExaminationValue::Image(_) => 5,
            ExaminationValue::Break => 6,
            ExaminationValue::Tabs(_) => 7,
            ExaminationValue::Countdown(_) => 8,
            ExaminationValue::Icon(_) => 9,
            ExaminationValue::LongInt(_) => 10,
            ExaminationValue::Effect(_) => 12,
            ExaminationValue::Other(k) => *k,
        }
}

pub fn convert(bytes: &[u8]) -> Option<Value> {
    let p = profile(bytes)?;
    let nodes: Vec<Value> = p
        .nodes
        .iter()
        .map(|n| {
            let mut out = json!({"kind":kind(&n.value),"appearance":n.appearance,"order":n.order});
            match &n.value {
                ExaminationValue::Text(s) => {
                    out["text"] = crate::events::string(&s)
                }
                ExaminationValue::Int(v) => out["value"] = json!(v),
                ExaminationValue::Float(v) => out["value"] = json!(v),
                ExaminationValue::Bool(v) => out["value"] = json!(v),
                ExaminationValue::LongInt(v) => out["value"] = json!(v.to_string()),
                ExaminationValue::Countdown(v) => out["value"] = json!(v),
                ExaminationValue::Image(v) => out["image"] = json!(v),
                ExaminationValue::Tabs(v) => out["value"] = json!(v),
                _ => (),
            }
            out
        })
        .collect();
    Some(json!({"event":"examination","id":p.request.iid.to_string(),"nodes":nodes}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admin_request_keeps_unsigned_id_and_has_no_legions_union_word() {
        let ClientEvent::Interp {
            function,
            arguments,
            ..
        } = request(0x814000000000002a)
        else {
            panic!()
        };
        assert_eq!(function, 0x820a0007);
        assert_eq!(arguments.len(), 44);
        assert_eq!(&arguments[32..40], &0x814000000000002au64.to_le_bytes());
        assert_eq!(&arguments[40..], &[0; 4]);
    }
    #[test]
    fn both_profile_layouts_decode_without_changing_names_or_ids() {
        let p = ExaminationProfile {
            request: ExaminationRequest::object(0x814000000000002a),
            nodes: vec![
                ExaminationDataNode {
                    order: 2,
                    appearance: 0xbe5b6513,
                    value: ExaminationValue::Text(StringInfo {
                        literal: Some("Training creature".into()),
                        ..Default::default()
                    }),
                },
                ExaminationDataNode {
                    order: 2,
                    appearance: 0,
                    value: ExaminationValue::Break,
                },
            ],
        };
        let bytes = pack_values(&[PackValue::native(NativeObject::ExaminationProfile(p))]);
        let modern = convert(&bytes).unwrap();
        let mut admin = bytes.clone();
        admin.drain(40..44);
        assert_eq!(convert(&admin).unwrap(), modern);
        assert_eq!(modern["id"], 0x814000000000002au64.to_string());
        assert_eq!(modern["nodes"][0]["text"]["literal"], "Training creature");
        for end in [0, 4, 24, 40, admin.len() - 1] {
            assert!(convert(&admin[..end]).is_none());
        }
        admin[20] = 0;
        assert!(convert(&admin).is_none());
    }
}
