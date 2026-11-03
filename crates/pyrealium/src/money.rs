//! The money bag's withdraw (`WM_Money` 0x8, function 0x820F0001): money out of the bag as a
//! stack of coins in the pack.
//!
//! The client (AC2_Rusted ac2-net `gameplay/calls.rs` `CMoneySystem` 0x5, `DragFromMoneyBag`;
//! the 7230 wlib's `WM_Money` stub f2) packs the target container (long int), the target slot
//! (int) and the quantity (int), and ghosts the bag until a `WM_Money` answer
//! (`CMoneySystem` 0x7 or 0x3, ac2-weenie `session/money.rs`) unghosts it.
//!
//! Retail (the 2017-02-12 captures, six withdraws, e.g.
//! `pkt-acct1-batch4/pkt_2017-1-23_1485231840_log.pcap`): the request names the player as the
//! container, slot 0 and the amount; the server answered in one batch with the coin item's
//! contain (0x100CB), `MoveItem_Done` (0x820E005E), the money (`UpdateInt` 0xC1C), the bag's
//! unghost (0x820F0006, the player) and the CreateObject of a coin stack holding the amount
//! (the relaunch DATs' coin entity; 7230's is `wsl_coin.entity`, 0x47001108, the same 84-byte
//! record). Here the coin goes into the pack through Pyrealium's own acquisition shape
//! (`item_services::acquired`, as for every item it grants) and the unghost is retail's
//! 0x820F0006 call, byte for byte.

use crate::inventory::{self, Inventory, Item, Location, BAG_SLOTS};
use crate::inventory_request::Metadata;
use crate::merchants::{MerchantError, Plan};
use crate::progression::{self, Progression};
use crate::world::WorldError;

/// `WM_Money` stub 0x8 (`CMoneySystem` 0x5): container, slot, quantity.
pub const WITHDRAW: u32 = 0x820f_0001;
/// `WM_Money` 0x6 -> `CMoneySystem` 0x7: the bag unghosts. One argument, the player.
pub const BAG_DONE: u32 = 0x820f_0006;
/// `wsl_coin.entity`.
pub const COIN: u32 = 0x4700_1108;
/// A coin stack's limit: the coin's weenie description holds 100,000,000 (see `tradeskills`'
/// stack notes beside the motes' 1,000,000).
pub const COIN_STACK: u32 = 100_000_000;

/// The coin's bag profile, read from the DAT once.
#[derive(Clone, Copy, Debug)]
pub struct Coin {
    pub visual: u32,
    pub metadata: Metadata,
}
impl Coin {
    pub fn load(dat: &mut crate::dat::Dat) -> std::io::Result<Self> {
        let metadata = crate::tradeskills::metadata(dat, COIN)?;
        let visual = crate::tradeskills::visual_of(dat, metadata.physical)?;
        if let Some(name) = crate::item_names::name_of(dat, COIN) {
            crate::item_names::register(COIN, name);
        }
        Ok(Self { visual, metadata })
    }
    /// A coin stack of `quantity`.
    pub fn item(&self, id: u64, quantity: u32) -> Item {
        Item {
            roll: None,
            id,
            definition: COIN,
            visual: self.visual,
            quantity,
            stack_limit: COIN_STACK,
            equipment_mask: 0,
            location: Location::Bag(0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawRequest {
    pub container: u64,
    /// The slot the drag named (`u32::MAX` for none).
    pub slot: u32,
    pub quantity: u32,
}
impl WithdrawRequest {
    /// Tagged container (2: two words), tagged slot (1), tagged quantity (1).
    pub fn decode(arguments: &[u8]) -> Result<Self, WorldError> {
        if arguments.len() != 28 {
            return Err(WorldError::InvalidValue);
        }
        let word = |at: usize| u32::from_le_bytes(arguments[at..at + 4].try_into().unwrap());
        if word(0) != 2 || word(12) != 1 || word(20) != 1 {
            return Err(WorldError::InvalidValue);
        }
        let container = u64::from(word(4)) | u64::from(word(8)) << 32;
        let request = Self { container, slot: word(16), quantity: word(24) };
        if [0, u64::MAX].contains(&container) || request.quantity == 0 {
            return Err(WorldError::InvalidValue);
        }
        Ok(request)
    }
}

/// `WM_Money` 0x6, retail's unghost: a private call with the player.
pub fn bag_done(owner: u64) -> Result<Vec<u8>, WorldError> {
    if [0, u64::MAX].contains(&owner) {
        return Err(WorldError::InvalidValue);
    }
    crate::script::private_call(BAG_DONE, &crate::script::identity(owner))
}

/// The withdraw of `request` from `owner`'s money: the coins (`id`) go first in the pack, the
/// gold leaves the bag, and the bag unghosts. Pyrealium policy: only the player's own pack
/// takes the coins (every retail capture names the player), the slot the drag named is not
/// kept (the pack is dense and a new item is listed first, `inventory::Command::GrantFront`),
/// and at most one coin stack's worth leaves at once.
pub fn plan_withdraw(
    owner: u64,
    request: &WithdrawRequest,
    coin: &Coin,
    inventory: &Inventory,
    state: &Progression,
    id: u64,
) -> Result<Plan, MerchantError> {
    if !state.valid() {
        return Err(MerchantError::InvalidState);
    }
    if !state.alive() {
        return Err(MerchantError::Dead);
    }
    if !inventory.dense_bags() {
        return Err(MerchantError::SparseBag);
    }
    if request.container != owner || request.quantity > COIN_STACK {
        return Err(MerchantError::InvalidIdentity);
    }
    if request.slot != u32::MAX && request.slot >= u32::from(BAG_SLOTS) {
        return Err(MerchantError::InvalidIdentity);
    }
    inventory.free_slot().ok_or(MerchantError::FullBag)?;
    let items = vec![inventory::Command::GrantFront(coin.item(id, request.quantity))];
    let changes = vec![progression::Command::SpendGold(request.quantity)];
    let (next_inventory, next_progression) =
        crate::player_transaction::candidate(inventory, state, &items, &changes, Some(id))
            .map_err(|_| MerchantError::InsufficientGold)?;
    Ok(Plan {
        items,
        changes,
        events: vec![bag_done(owner).map_err(|_| MerchantError::InvalidIdentity)?],
        message: format!("You take {} coins from your money bag.", request.quantity),
        next_inventory,
        next_progression,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: u64 = 0x2130_0000_0000_0dfd;

    #[test]
    fn the_withdraw_decodes_as_the_captured_request() {
        // pkt_2017-1-23_1485231840_log.pcap: 0x10053, 0x820F0001, 0x1C bytes of arguments.
        let wire = hex("02000000fd0d000000003021010000000000000001000000dc050000");
        let request = WithdrawRequest::decode(&wire).unwrap();
        assert_eq!(request, WithdrawRequest { container: PLAYER, slot: 0, quantity: 1500 });
        assert!(WithdrawRequest::decode(&wire[..24]).is_err());
        let mut zero = wire.clone();
        zero[24..28].copy_from_slice(&0u32.to_le_bytes());
        assert!(WithdrawRequest::decode(&zero).is_err());
    }

    #[test]
    fn the_unghost_is_the_captured_call_byte_for_byte() {
        // The same capture's server batch: 66 00 01 00, 0x820F0006, 0x0C bytes, the player.
        assert_eq!(
            bag_done(PLAYER).unwrap(),
            hex("6600010006000f820c00000002000000fd0d000000003021")
        );
    }

    #[test]
    fn a_withdraw_spends_the_gold_and_grants_one_coin_stack_first() {
        let coin = Coin {
            visual: 0x1f00_01cf,
            metadata: Metadata { implement: None, icon: 0, physical: 0x4700_0260, equipment: None },
        };
        let inventory = Inventory::default();
        let mut state = Progression::default();
        state = state.apply(&progression::Command::AwardGold(2000)).unwrap();
        let id = inventory::ITEM_NAMESPACE + 1;
        let request = WithdrawRequest { container: PLAYER, slot: 0, quantity: 1500 };
        let plan = plan_withdraw(PLAYER, &request, &coin, &inventory, &state, id).unwrap();
        assert_eq!(plan.next_progression.gold, 500);
        let granted: Vec<_> = plan.next_inventory.items().collect();
        assert_eq!(granted.len(), 1);
        assert_eq!((granted[0].definition, granted[0].quantity), (COIN, 1500));
        assert_eq!(plan.events, vec![bag_done(PLAYER).unwrap()]);
        // More than the bag holds, another container, a bad slot: refused.
        for bad in [
            WithdrawRequest { quantity: 2001, ..request.clone() },
            WithdrawRequest { container: PLAYER + 1, ..request.clone() },
            WithdrawRequest { slot: u32::from(BAG_SLOTS), ..request.clone() },
        ] {
            assert!(plan_withdraw(PLAYER, &bad, &coin, &inventory, &state, id).is_err());
        }
    }

    /// The 7230 portal's `wsl_coin` loads as a bag item (AC2_TEST_7230_PORTAL: the portal.dat).
    #[test]
    fn the_7230_coin_loads() {
        let Some(path) = std::env::var_os("AC2_TEST_7230_PORTAL") else { return };
        let mut dat = crate::dat::Dat::open(path).unwrap();
        let coin = Coin::load(&mut dat).unwrap();
        assert_eq!(coin.visual >> 24, 0x1f);
        assert_eq!(coin.metadata.physical >> 24, 0x47);
        let item = coin.item(inventory::ITEM_NAMESPACE + 1, 1500);
        crate::item_services::acquired(PLAYER, &item, coin.metadata).unwrap();
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }
}
