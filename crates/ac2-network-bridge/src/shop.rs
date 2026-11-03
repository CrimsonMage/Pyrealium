// Native WM_Store graphs. No client-supplied prices or item definitions on purchases.
use ac2_net::protocol::*;
use serde_json::{Value, json};

pub fn decode(function: u32, arguments: &[u8]) -> Option<Value> {
    Some(match StoreMessage::decode(function, arguments)? {
        Ok(StoreMessage::ViewStore { view, vendor })
        | Ok(StoreMessage::UpdateStore { view, vendor }) => {
            json!({"event":"shop","vendor":vendor.to_string(),"template":view.store,"offers":view.sales.iter().map(|s|
                json!({"id":s.item.to_string(),"entity":s.item_did,"icon":s.icon_did,"price":s.cost,"currency":s.cost_did,"stock":s.sold,
                    "name":{"literal":s.item_desc.literal,"table":s.item_desc.table_id,"key":s.item_desc.string_id}})).collect::<Vec<_>>()})
        }
        Ok(StoreMessage::LeaveStore { vendor }) => {
            json!({"event":"shop_close","vendor":vendor.to_string()})
        }
        Ok(StoreMessage::UpdateSale {
            vendor,
            item,
            stock,
        }) => {
            json!({"event":"shop_stock","vendor":vendor.to_string(),"id":item.to_string(),"stock":stock})
        }
        Ok(_) => {
            json!({"event":"error","message":"This vendor's catalog/consignment service is not supported yet."})
        }
        Err(error) => json!({"event":"error","message":format!("Invalid native shop: {error}")}),
    })
}
pub fn buy(owner: u64, vendor: u64, item: u64) -> ClientEvent {
    // Captured TransactionBlob 3812 has ten fields, including explicit owner.
    // The reference's convenience Purchase uses a different nine-field profile.
    let mut arguments = Vec::new();
    for word in [
        5u32,
        2,
        1,
        2,
        0x44000100,
        0,
        0x38120000,
        10,
        item as u32,
        (item >> 32) as u32,
        2,
        owner as u32,
        (owner >> 32) as u32,
        1,
        vendor as u32,
        (vendor >> 32) as u32,
        0,
        0,
    ] {
        arguments.extend(word.to_le_bytes());
    }
    arguments.extend([0x84, 0xcd, 1, 0x84, 0xcd, 4, 0x82, 0xcd, 2, 4, 0, 0]);
    for word in [0x44000100u32, 0, 0xffff003b, 0, 0] {
        arguments.extend(word.to_le_bytes());
    }
    ClientEvent::Interp {
        function: 0xb83a000b,
        arguments,
        encrypted: false,
    }
}
pub fn sell(owner: u64, item: u64, slot: u32, quantity: u32) -> ClientEvent {
    ClientEvent::Interp {
        function: 0x820f0002,
        encrypted: false,
        arguments: pack_values(&[
            PackValue::LongInt(owner),
            PackValue::Int(slot),
            PackValue::Int(quantity),
            PackValue::LongInt(item),
        ]),
    }
}
pub fn close(vendor: u64) -> ClientEvent {
    ClientEvent::Interp {
        function: 0xb83a0003,
        encrypted: false,
        arguments: pack_values(&[PackValue::LongInt(vendor)]),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn purchase_and_sale_preserve_full_native_identity() {
        let ClientEvent::Interp { arguments, .. } = buy(7, 0x8140000000030003, 0x8160000000000001)
        else {
            panic!()
        };
        assert_eq!(arguments.len(), 104);
        assert_eq!(&arguments[32..40], &0x8160000000000001u64.to_le_bytes());
        let ClientEvent::Interp { arguments, .. } = sell(7, 0x8130000000000001, 2, 1) else {
            panic!()
        };
        assert_eq!(arguments.len(), 40);
        assert_eq!(&arguments[32..40], &0x8130000000000001u64.to_le_bytes());
    }
}
