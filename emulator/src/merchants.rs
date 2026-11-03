//! Authored development quartermaster transactions and a bounded native sale.
//!
//! Prices/stock are server policy, not recovered original economy. The retained
//! nested shopping capture establishes the native catalog and purchase graphs.
//! Fresh bounded graphs use server-owned offer identities and fixed stock.
//! Evidence: npc.interaction/shopping.elog, ikeras.weapons.vendor.buying.weapon.elog;
//! converted build-7148 WLib 56000005 b83a0013 at 102780, b8320007 at 11bdb0,
//! b8320019 at 11c57c; native UIShop adapter 00467ce0. Captured client build
//! identity is unknown; matching bytecode corroborates field access and UI calls.
//! Native client acceptance of newly authored stock remains a runtime boundary.
//! Native 820f0002/820f0009 single-item sale fields follow the retained sale
//! capture. That request has no merchant IID or session nonce: the caller must
//! establish and revalidate a nearby merchant context before accepting it.
use crate::{
    dat::Dat,
    inventory::{self, BAG_SLOTS, Inventory, Item, Location},
    inventory_request::Metadata,
    progression::{self, Progression},
    world::WorldError,
};
use std::{collections::BTreeMap, fmt, io};

/// A DAT-backed humanoid reused for a separately authored training placement.
pub const MERCHANT_ENTITY: u32 = 0x47003724;
pub const MERCHANT_VISUAL: u32 = 0x1f001941;
pub const SELL: u32 = 0x820f0002;
pub const SALE_RESULT: u32 = 0x820f0009;
pub const VIEW_STORE: u32 = 0xb83a0013;
pub const REFRESH_STORE: u32 = 0xb83a002a;
pub const BUY: u32 = 0xb83a000b;
pub const LEAVE: u32 = 0xb83a0003;
pub const CLOSE: u32 = 0xb83a0010;

/// Native LeaveStore updates only the matching active merchant context.
pub fn close(merchant: u64) -> Result<Vec<u8>, WorldError> {
    if [0, u64::MAX].contains(&merchant) {
        return Err(WorldError::InvalidValue);
    }
    crate::script::private_call(CLOSE, &crate::script::identity(merchant))
}
/// Matching interpreted layouts; callers must verify their active WLib.
pub const LAYOUTS: [(usize, u32); 3] = [(0x385d, 4), (0x382e, 11), (0x3812, 10)];
const STORE_TEMPLATE: u32 = 0x6f00609a;
const GOLD_DEFINITION: u32 = 0x47001108;
pub const STOCK: [Stock; 16] = [Stock::Potion, Stock::Weapon, Stock::Mucor, Stock::Tallow, Stock::Material(0x470010f2),Stock::Material(0x470010f3),Stock::Material(0x470010f4),Stock::Material(0x470010f5),Stock::Material(0x470010fe),Stock::Material(0x470027a0),Stock::Material(0x470027a3),Stock::Material(0x4700141f),Stock::Material(0x47001400),Stock::Material(0x47002ad4),Stock::Material(0x470010fa),Stock::Material(0x470027a2)];
/// Shared bounded Pyrealium price policy; unknown/quest items cannot be transmuted.
pub fn sale_value(item:&Item)->Option<(&'static str,u32)> {
    if let Some(stock)=Stock::for_item(item) {Some((stock.name(),stock.sell_price()))}
    else if crate::starter_gear::tradable(item) || crate::apparel::tradable(item) || crate::treasure_weapons::valid_finish(item) {Some(("equipment",3))}
    else {None}
}
pub const MUCOR_DEFINITION: u32 = 0x47001744;
pub const MUCOR_VISUAL: u32 = 0x1f000e37;
pub const MUCOR_PHYSICAL: u32 = 0x4700176b;
pub const MUCOR_ICON: u32 = 0x20000021;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stock {
    Potion,
    Weapon,
    Mucor,
    Tallow,
    Material(u32),
}
impl Stock {
    pub fn name(self) -> &'static str {
        match self {
            Self::Potion => "potion",
            Self::Weapon => "weapon",
            Self::Mucor => "mushroom",
            Self::Tallow => "tallow",
            Self::Material(d)=>match d{0x470010f2=>"Crystal (100)",0x470010f3=>"Iron (100)",0x470010f4=>"Stone (100)",0x470010f5=>"Wood (100)",0x470010fe=>"Silver (100)",0x470027a0=>"Chitin (100)",0x470027a3=>"Hide (100)",0x4700141f=>"Sinew (100)",0x47001400=>"Bone (100)",0x47002ad4=>"Coal (100)",0x470010fa=>"Flawless Pyreal (100)",0x470027a2=>"Gathered Deruwood (100)",_=>"Material"},
        }
    }
    pub fn definition(self) -> u32 {
        match self {
            Self::Potion => crate::item_services::POTION_DEFINITION,
            Self::Weapon => crate::training_kit::DEFINITION,
            Self::Mucor => MUCOR_DEFINITION,
            Self::Tallow => crate::crafting::CATALYST,
            Self::Material(d)=>d,
        }
    }
    pub fn buy_price(self) -> u32 {
        match self {
            Self::Potion => 5,
            Self::Weapon => 12,
            Self::Mucor => 2,
            Self::Tallow => 2,
            Self::Material(_)=>5,
        }
    }
    pub fn sell_price(self) -> u32 {
        match self {
            Self::Potion => 1,
            Self::Weapon => 3,
            Self::Mucor => 1,
            Self::Tallow => 1,
            Self::Material(_)=>1,
        }
    }
    pub fn item(self, id: u64, slot: u16) -> Item {
        match self {
            Self::Potion => crate::item_services::potion(id, slot),
            Self::Weapon => crate::training_kit::item(id, slot),
            Self::Mucor => mucor(id, slot),
            Self::Tallow => crate::crafting::tallow(id, slot),
            Self::Material(d)=>Item{id,definition:d,visual:crate::professions::catalog().items.iter().find(|i|i.id==d).expect("supplier material").visual,quantity:100,stack_limit:u32::from(u16::MAX),equipment_mask:0,location:Location::Bag(slot),roll:None},
        }
    }
    fn by_name(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("mucor") {
            return Some(Self::Mucor);
        }
        [Self::Potion, Self::Weapon, Self::Mucor, Self::Tallow]
            .into_iter()
            .find(|stock| value.eq_ignore_ascii_case(stock.name()))
    }
    fn for_item(item: &Item) -> Option<Self> {
        let Location::Bag(slot) = item.location else {
            return None;
        };
        [Self::Potion, Self::Weapon, Self::Mucor, Self::Tallow]
            .into_iter()
            .find(|stock| *item == stock.item(item.id, slot))
            .or_else(|| {
                (item.definition == crate::crafting::TALLOW
                    && item.visual == 0x1f000234
                    && item.quantity == 1
                    && item.stack_limit == 1
                    && item.equipment_mask == 0)
                    .then_some(Self::Tallow)
            })
    }
}

pub struct Catalog {
    metadata: BTreeMap<u32, Metadata>,
    store_template:u32,
}
impl Catalog {
    pub fn validate(portal: &mut Dat) -> io::Result<Self> {Self::validate_profile(portal,false)}
    pub fn validate_legions(portal:&mut Dat)->io::Result<Self>{Self::validate_profile(portal,true)}
    fn validate_profile(portal: &mut Dat, legions:bool) -> io::Result<Self> {
        // The retained catalog loads this native StoreTemplate for its title,
        // description and portrait. Stock and prices below are authored policy.
        let store_template=if legions {0x6f0036e8}else{STORE_TEMPLATE};
        let template = portal.read(store_template)?;
        let gold = portal.read(GOLD_DEFINITION)?;
        let fingerprint = |bytes: &[u8]| {
            bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            })
        };
        if template.len() != if legions {1996}else{760}
            || fingerprint(&template) != if legions {0xbf0c05857e7e02fd}else{0x67688aec506e3138}
            || gold.len() != 84
            || fingerprint(&gold) != 0x8f7c1215933eab75
        {
            return Err(io::Error::other(
                "unsupported native store template or gold definition",
            ));
        }
        for texture in [0x41006f81, 0x4100432b, 0x41006fb0, 0x410082d6] {
            portal.read(texture)?;
        }
        portal.read(MERCHANT_ENTITY)?;
        portal.read(MERCHANT_VISUAL)?;
        let potion = crate::item_services::validate_potion(portal)?;
        let weapon = crate::training_kit::validate(portal)?;
        let mucor = validate_mucor(portal)?;
        let tallow = crate::crafting::validate_tallow(portal)?;
        let mut metadata=BTreeMap::from([
                (Stock::Potion.definition(), potion),
                (Stock::Weapon.definition(), weapon),
                (Stock::Mucor.definition(), mucor),
                (Stock::Tallow.definition(), tallow),
            ]);
        if legions {for stock in STOCK.iter().skip(4){let d=stock.definition();metadata.insert(d,Metadata::load(portal,d)?);}}
        Ok(Self{store_template,metadata})
    }
    pub fn metadata(&self, definition: u32) -> Option<Metadata> {
        self.metadata.get(&definition).copied()
    }
    pub fn entries(&self) -> impl Iterator<Item = (u32, Metadata)> + '_ {
        self.metadata
            .iter()
            .map(|(&definition, &metadata)| (definition, metadata))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    List,
    Buy(Stock),
    /// Internal zero-based dense bag index; command text uses slots 1..78.
    SellSlot(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MerchantError {
    Command,
    Dead,
    InvalidState,
    SparseBag,
    FullBag,
    NotOwned,
    NotForSale,
    InsufficientGold,
    InvalidIdentity,
}
impl fmt::Display for MerchantError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Command => "Use @shop, @buy potion|weapon|mushroom|tallow, or @sell SLOT (bag slots 1..78).",
            Self::Dead => "You cannot trade while dead.",
            Self::InvalidState => "The current inventory or gold balance cannot accept this trade.",
            Self::SparseBag => "This trade requires a dense bag list. Sort or restore your bag first.",
            Self::FullBag => "You need a free bag slot before buying an item.",
            Self::NotOwned => "There is no owned bag item in that slot.",
            Self::NotForSale => "This merchant buys supported stock, starter gear and recovered armor. Unequip an item before selling it; quest items cannot be sold.",
            Self::InsufficientGold => "You do not have enough gold for that item.",
            Self::InvalidIdentity => "The trade owner, item identity or bag position is invalid.",
        })
    }
}
impl std::error::Error for MerchantError {}

/// None leaves ordinary chat untouched; recognized malformed commands return a
/// readable error instead of being broadcast or partially interpreted.
pub fn parse_command(text: &str) -> Option<Result<Action, MerchantError>> {
    let mut words = text.split_whitespace();
    let verb = words.next()?;
    if !["@shop", "@buy", "@sell"]
        .iter()
        .any(|expected| verb.eq_ignore_ascii_case(expected))
    {
        return None;
    }
    if text.len() > 128 {
        return Some(Err(MerchantError::Command));
    }
    let argument = words.next();
    if words.next().is_some() {
        return Some(Err(MerchantError::Command));
    }
    Some(match (verb, argument) {
        (verb, None) if verb.eq_ignore_ascii_case("@shop") => Ok(Action::List),
        (verb, Some(name)) if verb.eq_ignore_ascii_case("@buy") => Stock::by_name(name)
            .map(Action::Buy)
            .ok_or(MerchantError::Command),
        (verb, Some(slot)) if verb.eq_ignore_ascii_case("@sell") => slot
            .parse::<u16>()
            .ok()
            .filter(|slot| (1..=BAG_SLOTS).contains(slot))
            .map(|slot| Action::SellSlot(slot - 1))
            .ok_or(MerchantError::Command),
        _ => Err(MerchantError::Command),
    })
}

pub fn listing() -> String {
    "Welcome. Select an item and press Purchase to buy it. Drag an unequipped supported item from your backpack onto the shop to sell it. This server currently supplies a limited stock of potions, weapons and crafting materials.".into()
}

pub fn mucor(id: u64, slot: u16) -> Item {
    Item {roll: None,
        id,
        definition: MUCOR_DEFINITION,
        visual: MUCOR_VISUAL,
        quantity: 1,
        stack_limit: 1,
        equipment_mask: 0,
        location: Location::Bag(slot),
    }
}

/// The Mucor descriptor uses a crafting-description bit outside the generic
/// Metadata reader. Pin the complete known records rather than relaxing that
/// reader or searching arbitrary bytes for a plausible icon/physical DID.
pub fn validate_mucor(portal: &mut Dat) -> io::Result<Metadata> {
    fn fingerprint(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
    }
    let ui = portal.read(0x81001744)?;
    let physical = portal.read(MUCOR_PHYSICAL)?;
    if ui.len() != 188
        || fingerprint(&ui) != 0xbd624233466cca50
        || physical.len() != 44
        || fingerprint(&physical) != 0x197289ce0498f131
    {
        return Err(io::Error::other(
            "unsupported Mucor inventory metadata profile",
        ));
    }
    for did in [MUCOR_DEFINITION, MUCOR_VISUAL, MUCOR_ICON] {
        portal.read(did)?;
    }
    Ok(Metadata {
        implement: None,
        icon: MUCOR_ICON,
        physical: MUCOR_PHYSICAL,
        equipment: None,
    })
}

/// Caller admits the complete replication batch before saving items+payment in
/// one Store::player_transaction. These events are supplemental acknowledgments;
/// ordinary item creation/acquisition/removal and private gold updates are still
/// required through the shared item-service replication path.
pub struct Plan {
    pub items: Vec<inventory::Command>,
    pub changes: Vec<progression::Command>,
    pub events: Vec<Vec<u8>>,
    pub message: String,
    pub next_inventory: Inventory,
    pub next_progression: Progression,
}

fn available(inventory: &Inventory, state: &Progression) -> Result<(), MerchantError> {
    if !state.valid() {
        return Err(MerchantError::InvalidState);
    }
    if !state.alive() {
        return Err(MerchantError::Dead);
    }
    if !inventory.dense_bags() {
        return Err(MerchantError::SparseBag);
    }
    Ok(())
}

pub fn plan_buy(
    stock: Stock,
    inventory: &Inventory,
    state: &Progression,
    next_item: u64,
) -> Result<Plan, MerchantError> {
    available(inventory, state)?;
    if state.gold < stock.buy_price() {
        return Err(MerchantError::InsufficientGold);
    }
    let mut product = stock.item(next_item, 0);
    let mut items = Vec::new();
    let mut bag = inventory.clone();
    // Ordinary supply materials accumulate in a compatible stack, including
    // when every bag slot is occupied. Preserve the exact Tallow lesson bundle.
    if matches!(stock, Stock::Material(_)) {
        if let Some(stack) = inventory.items().find(|i| {
            i.definition == product.definition && i.visual == product.visual
                && i.roll.is_none() && i.equipment_mask == 0
                && i.stack_limit == product.stack_limit
                && matches!(i.location, Location::Bag(_))
                && i.quantity.checked_add(product.quantity).is_some()
        }) {
            let consume = inventory::Command::ConsumeStackDense { item: stack.id, quantity: stack.quantity };
            bag = bag.apply(&consume).map_err(|_| MerchantError::InvalidState)?;
            product.quantity += stack.quantity;
            items.push(consume);
        }
    }
    product.location = Location::Bag(bag.free_slot().ok_or(MerchantError::FullBag)?);
    items.push(inventory::Command::Grant(product));
    let changes = vec![progression::Command::SpendGold(stock.buy_price())];
    let (next_inventory, next_progression) =
        crate::player_transaction::candidate(inventory, state, &items, &changes, Some(next_item))
            .map_err(|_| MerchantError::InvalidState)?;
    Ok(Plan {
        items,
        changes,
        events: Vec::new(),
        message: format!("Bought {} for {} gold.", stock.name(), stock.buy_price()),
        next_inventory,
        next_progression,
    })
}

pub fn plan_sell(
    slot: u16,
    inventory: &Inventory,
    state: &Progression,
) -> Result<Plan, MerchantError> {
    available(inventory, state)?;
    if slot >= BAG_SLOTS {
        return Err(MerchantError::InvalidIdentity);
    }
    let item = inventory
        .items()
        .find(|item| item.location == Location::Bag(slot))
        .ok_or(MerchantError::NotOwned)?;
    let (name,price)=sale_value(item).ok_or(MerchantError::NotForSale)?;
    let items = vec![if item.quantity > 1 {
        inventory::Command::ConsumeStackDense {
            item: item.id,
            quantity: item.quantity,
        }
    } else {
        inventory::Command::ConsumeDense { item: item.id }
    }];
    let changes = vec![progression::Command::AwardGold(price)];
    let (next_inventory, next_progression) =
        crate::player_transaction::candidate(inventory, state, &items, &changes, None)
            .map_err(|_| MerchantError::InvalidState)?;
    Ok(Plan {
        items,
        changes,
        events: Vec::new(),
        message: format!("Sold {name} for {price} gold."),
        next_inventory,
        next_progression,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SellRequest {
    pub owner: u64,
    pub item: u64,
    pub ordinal: u16,
    pub quantity: u32,
}
impl SellRequest {
    /// Tagged owner, ordinal, complete supported quantity and tagged item.
    /// There are no client price, merchant, or session-nonce fields. A request
    /// must match the whole owned stack; partial stack sales are unsupported.
    pub fn decode(arguments: &[u8]) -> Result<Self, WorldError> {
        if arguments.len() != 40 {
            return Err(WorldError::InvalidValue);
        }
        let word = |offset| u32::from_le_bytes(arguments[offset..offset + 4].try_into().unwrap());
        let owner = u64::from(word(4)) | u64::from(word(8)) << 32;
        let item = u64::from(word(32)) | u64::from(word(36)) << 32;
        if word(0) != 2
            || word(12) != 1
            || word(20) != 1
            || ![1, u32::from(crate::crafting::TALLOW_QUANTITY)].contains(&word(24))
            || word(28) != 2
            || word(16) >= u32::from(BAG_SLOTS)
            || [0, u64::MAX].contains(&owner)
            || [0, u64::MAX].contains(&item)
            || owner == item
        {
            return Err(WorldError::InvalidValue);
        }
        Ok(Self {
            owner,
            item,
            ordinal: word(16) as u16,
            quantity: word(24),
        })
    }
}

pub fn plan_native_sell(
    owner: u64,
    request: &SellRequest,
    inventory: &Inventory,
    state: &Progression,
) -> Result<Plan, MerchantError> {
    if request.owner != owner
        || !inventory.items().any(|item| {
            item.id == request.item
                && item.location == Location::Bag(request.ordinal)
                && item.quantity == request.quantity
        })
    {
        return Err(MerchantError::InvalidIdentity);
    }
    let mut plan = plan_sell(request.ordinal, inventory, state)?;
    plan.events
        .push(sale_result(owner, request.ordinal).map_err(|_| MerchantError::InvalidIdentity)?);
    Ok(plan)
}

pub fn sale_result(owner: u64, ordinal: u16) -> Result<Vec<u8>, WorldError> {
    if [0, u64::MAX].contains(&owner) || ordinal >= BAG_SLOTS {
        return Err(WorldError::InvalidValue);
    }
    let mut arguments = crate::script::identity(owner);
    arguments.extend(crate::script::integer(u32::from(ordinal)));
    arguments.extend(crate::script::identity(0));
    crate::script::private_call(SALE_RESULT, &arguments)
}

/// One server-issued catalog context. Allocate fresh offer IIDs whenever this
/// context is replaced, and expire it on logout, death, or leaving trade range.
/// The caller revalidates range and commits inventory/gold and queued UI events
/// atomically. Client graph references never become owned inventory identities.
#[derive(Clone, Debug)]
pub struct Shop {
    merchant: u64,
    offers: Vec<u64>,
    /// A trader's own store (the DAT's `store_*.wc` lines), else the development quartermaster's four.
    dat: Option<DatShop>,
}
/// A trader's store: its record (the shop's template) and its lines with their name objects.
#[derive(Clone, Debug)]
pub struct DatShop {
    pub store: u32,
    pub sales: Vec<(crate::tradeskills::StoreLine, Vec<u8>)>,
}
impl Shop {
    /// The shop of a trader whose store record the DAT holds (one offer identity a line).
    pub fn dat(merchant: u64, offers: Vec<u64>, store: DatShop) -> Result<Self, WorldError> {
        let shop = Self::new_with(merchant, offers)?;
        if shop.offers.len() != store.sales.len() || store.sales.is_empty() {
            return Err(WorldError::InvalidValue);
        }
        Ok(Self { dat: Some(store), ..shop })
    }
    pub fn new(merchant: u64, offers: impl Into<Vec<u64>>) -> Result<Self, WorldError> {
        let offers = offers.into();
        if offers.is_empty() || offers.len() > STOCK.len() {
            return Err(WorldError::InvalidValue);
        }
        Self::new_with(merchant, offers)
    }
    fn new_with(merchant: u64, offers: Vec<u64>) -> Result<Self, WorldError> {
        if [0, u64::MAX].contains(&merchant)
            || offers.iter().enumerate().any(|(index, id)| {
                [0, u64::MAX, merchant].contains(id) || offers[..index].contains(id)
            })
        {
            return Err(WorldError::InvalidValue);
        }
        Ok(Self { merchant, offers, dat: None })
    }

    pub fn merchant(&self) -> u64 {
        self.merchant
    }

    /// Whether this is a trader's own store (see [`DatShop`]).
    pub fn is_trader(&self) -> bool {
        self.dat.is_some()
    }

    /// b83a0013 -> 1aa401de -> b8320006/b8320007 -> 38390006.
    /// Native UIShop is constructed by client bytecode, never packed as an
    /// invented native C++ heap object. The final IID is the merchant target;
    /// the client obtains the active player through EFun 00720014.
    pub fn open(&self, _validated_catalog: &Catalog) -> Result<Vec<u8>, WorldError> {
        if let Some(dat) = &self.dat {
            return self.open_dat(dat);
        }
        let mut objects = vec![ShopObject::interpreted(
            0x385d,
            vec![(2, 1), (_validated_catalog.store_template, 4), (self.offers.len()as u32, 4), (0, 2)],
        )];
        let mut list = (self.offers.len()as u32).to_le_bytes().to_vec();
        for index in 0..self.offers.len()as u32 {
            list.extend((3 + index * 2u32).to_le_bytes());
        }
        objects.push(ShopObject::native(0x65, list));
        for (index, stock) in STOCK.into_iter().take(self.offers.len()).enumerate() {
            let identity = self.offers[index];
            let texture = match stock {
                Stock::Potion => 0x41006f81,
                // Foreground icon layers in the validated physical visuals.
                Stock::Weapon => 0x4100432b,
                Stock::Mucor => 0x41006fb0,
                Stock::Tallow => 0x410082d6,
                Stock::Material(_)=>0,
            };
            objects.push(ShopObject::interpreted(
                0x382e,
                vec![
                    (stock.definition(), 4),
                    (u32::MAX, 1),
                    (identity as u32, 0x84),
                    ((identity >> 32) as u32, 0xcd),
                    (texture, 4),
                    (4 + index as u32 * 2, 1),
                    // Native SetCostQuantity 0046d9e0 copies this raw f32;
                    // captured 382e prices use storage tag 4, not tag 3.
                    ((stock.buy_price() as f32).to_bits(), 4),
                    (GOLD_DEFINITION, 4),
                    (1, 4), // Advertise the supported single-item quantity.
                    (index as u32, 2),
                    (0, 2),
                ],
            ));
            objects.push(ShopObject::native(
                0x6a,
                crate::world::literal_name(match stock {
                    Stock::Potion => "Health Potion",
                    Stock::Weapon => "Training Weapon",
                    Stock::Mucor => "Mucor Mushroom",
                    Stock::Tallow => "Tallow Bundle (100)",
                    Stock::Material(_)=>stock.name(),
                })?,
            ));
        }
        let mut arguments = shop_graph(&objects);
        arguments.extend(crate::script::identity(self.merchant));
        crate::script::private_call(VIEW_STORE, &arguments)
    }

    /// A trader's store: the same graph with the store record as its template (the record is a
    /// `StoreTemplate`: its title, description and portrait) and one sale per record line, priced
    /// per unit as the record states.
    fn open_dat(&self, dat: &DatShop) -> Result<Vec<u8>, WorldError> {
        let count = dat.sales.len() as u32;
        let mut objects = vec![ShopObject::interpreted(
            0x385d,
            vec![(2, 1), (dat.store, 4), (count, 4), (0, 2)],
        )];
        let mut list = count.to_le_bytes().to_vec();
        for index in 0..count {
            list.extend((3 + index * 2).to_le_bytes());
        }
        objects.push(ShopObject::native(0x65, list));
        for (index, (line, name)) in dat.sales.iter().enumerate() {
            let identity = self.offers[index];
            objects.push(ShopObject::interpreted(
                0x382e,
                vec![
                    (line.definition, 4),
                    (u32::MAX, 1),
                    (identity as u32, 0x84),
                    ((identity >> 32) as u32, 0xcd),
                    (line.icon, 4),
                    (4 + index as u32 * 2, 1),
                    (line.price.to_bits(), 4),
                    (GOLD_DEFINITION, 4),
                    // The quantity the line advertises: a stacking material sells by the unit.
                    (if crate::tradeskills::is_mote(line.definition) { 1000 } else { 1 }, 4),
                    (index as u32, 2),
                    (0, 2),
                ],
            ));
            objects.push(ShopObject::native(0x6a, name.clone()));
        }
        let mut arguments = shop_graph(&objects);
        arguments.extend(crate::script::identity(self.merchant));
        crate::script::private_call(VIEW_STORE, &arguments)
    }

    /// Buy `request.quantity` units of a trader's line: the units are one stack (or, for an item that
    /// does not stack, the request must be for one), paid in whole coins rounded up
    /// (`ceil(price * quantity)`). `make` builds the item (definition, id, slot, quantity).
    pub fn plan_buy_dat(
        &self,
        owner: u64,
        request: &BuyRequest,
        inventory: &Inventory,
        state: &Progression,
        item_id: u64,
        make: impl Fn(u32, u64, u16, u32) -> Option<Item>,
    ) -> Result<Plan, MerchantError> {
        let dat = self.dat.as_ref().ok_or(MerchantError::NotForSale)?;
        if request.owner != owner
            || request.merchant != self.merchant
            || owner == self.merchant
            || self.offers.contains(&item_id)
            || [owner, self.merchant].contains(&item_id)
        {
            return Err(MerchantError::InvalidIdentity);
        }
        let index = self
            .offers
            .iter()
            .position(|&id| id == request.offer)
            .ok_or(MerchantError::NotForSale)?;
        let (line, _) = &dat.sales[index];
        available(inventory, state)?;
        let slot = inventory.free_slot().ok_or(MerchantError::FullBag)?;
        let quantity = request.quantity.max(1);
        let item = make(line.definition, item_id, slot, 1).ok_or(MerchantError::NotForSale)?;
        if quantity > item.stack_limit {
            return Err(MerchantError::NotForSale);
        }
        let price = (f64::from(line.price) * f64::from(quantity)).ceil() as u32;
        if state.gold < price {
            return Err(MerchantError::InsufficientGold);
        }
        let item = Item { quantity, ..item };
        let items = vec![inventory::Command::Grant(item)];
        let changes = vec![progression::Command::SpendGold(price)];
        let (next_inventory, next_progression) =
            crate::player_transaction::candidate(inventory, state, &items, &changes, Some(item_id))
                .map_err(|_| MerchantError::InvalidState)?;
        Ok(Plan {
            items,
            changes,
            events: Vec::new(),
            message: format!("Bought {quantity} for {price} gold."),
            next_inventory,
            next_progression,
        })
    }

    /// The retained shopping capture sends the same store/list/sale graph and
    /// merchant tail through b83a002a after purchases. Matching WLib dispatches
    /// 1aa401e2 -> b8320008 -> b8320007, then UI update EFun 38390001.
    pub fn refresh(&self, validated_catalog: &Catalog) -> Result<Vec<u8>, WorldError> {
        let mut bytes = self.open(validated_catalog)?;
        bytes[4..8].copy_from_slice(&REFRESH_STORE.to_le_bytes());
        Ok(bytes)
    }

    pub fn plan_buy(
        &self,
        owner: u64,
        request: &BuyRequest,
        inventory: &Inventory,
        state: &Progression,
        item_id: u64,
    ) -> Result<Plan, MerchantError> {
        if request.owner != owner
            || request.merchant != self.merchant
            || owner == self.merchant
            || self.offers.contains(&item_id)
            || [owner, self.merchant].contains(&item_id)
        {
            return Err(MerchantError::InvalidIdentity);
        }
        let index = self
            .offers
            .iter()
            .position(|&id| id == request.offer)
            .ok_or(MerchantError::NotForSale)?;
        if self.dat.is_some() || request.quantity != 1 {
            return Err(MerchantError::NotForSale);
        }
        plan_buy(STOCK[index], inventory, state, item_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuyRequest {
    pub owner: u64,
    pub merchant: u64,
    pub offer: u64,
    /// The units asked for (the 3812 graph's tag-4 field at byte 52); one for the quartermaster.
    pub quantity: u32,
}
impl BuyRequest {
    /// Captured b83a000b: exact 3812 ten-field graph and empty native 3b
    /// selection. Only quantity one is supported. All references, tags, unused
    /// fields and object boundaries are checked; client prices are absent.
    pub fn decode(arguments: &[u8]) -> Result<Self, WorldError> {
        if arguments.len() != 104 {
            return Err(WorldError::InvalidValue);
        }
        let w = |offset| u32::from_le_bytes(arguments[offset..offset + 4].try_into().unwrap());
        let iid = |offset| u64::from(w(offset)) | u64::from(w(offset + 4)) << 32;
        let root = w(8);
        let selection = w(12);
        if w(0) != 5
            || w(4) != 2
            || [0, u32::MAX].contains(&root)
            || [0, u32::MAX, root].contains(&selection)
            || w(16) != 0x44000100
            || w(20) != 0
            || w(24) != 0x38120000
            || w(28) != 10
            || w(40) != selection
            || w(52) == 0
            || w(64) != 0
            || w(68) != 0
            || arguments[72..82] != [0x84, 0xcd, 1, 0x84, 0xcd, 4, 0x82, 0xcd, 2, 4]
            || w(84) != 0x44000100
            || w(88) != 0
            || w(92) != 0xffff003b
            || w(96) != 0
            || w(100) != 0
        {
            return Err(WorldError::InvalidValue);
        }
        let request = Self {
            offer: iid(32),
            owner: iid(44),
            merchant: iid(56),
            quantity: w(52),
        };
        if [request.offer, request.owner, request.merchant]
            .iter()
            .any(|id| [0, u64::MAX].contains(id))
            || request.offer == request.owner
            || request.offer == request.merchant
            || request.owner == request.merchant
        {
            return Err(WorldError::InvalidValue);
        }
        Ok(request)
    }
}

struct ShopObject {
    kind: u32,
    native: Vec<u8>,
    fields: Vec<(u32, u8)>,
}
impl ShopObject {
    fn interpreted(package: u16, fields: Vec<(u32, u8)>) -> Self {
        Self {
            kind: u32::from(package) << 16,
            native: Vec::new(),
            fields,
        }
    }
    fn native(kind: u16, native: Vec<u8>) -> Self {
        Self {
            kind: 0xffff0000 | u32::from(kind),
            native,
            fields: Vec::new(),
        }
    }
}
fn shop_graph(objects: &[ShopObject]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let word = |bytes: &mut Vec<u8>, value: u32| bytes.extend(value.to_le_bytes());
    word(&mut bytes, 5);
    word(&mut bytes, objects.len() as u32);
    for reference in 1..=objects.len() {
        word(&mut bytes, reference as u32);
    }
    for object in objects {
        for value in [0x44000100, 0, object.kind] {
            word(&mut bytes, value);
        }
        bytes.extend(&object.native);
        word(&mut bytes, object.fields.len() as u32);
        for &(value, _) in &object.fields {
            word(&mut bytes, value);
        }
        for &(_, tag) in &object.fields {
            bytes.push(tag);
        }
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }
    bytes
}

#[cfg(test)]
mod supply_tests {
    use super::*;
    #[test]
    fn material_purchase_merges_in_full_bag_and_rejects_overflow_without_payment() {
        let stock = Stock::Material(0x470010fe);
        let mut bag = Inventory::default();
        let mut silver = stock.item(inventory::ITEM_NAMESPACE + 1, 0);
        silver.quantity = 65_400;
        bag = bag.apply(&inventory::Command::Grant(silver.clone())).unwrap();
        for slot in 1..BAG_SLOTS {
            bag = bag.apply(&inventory::Command::Grant(mucor(bag.high_water()+1, slot))).unwrap();
        }
        let gold = Progression { gold: 100, ..Default::default() };
        let plan = plan_buy(stock, &bag, &gold, bag.high_water()+1).unwrap();
        let new = plan.next_inventory.items().find(|i| i.definition == silver.definition).unwrap();
        assert_eq!(new.quantity, 65_500);
        assert_ne!(new.id, silver.id, "Old stack identities must become stale");
        assert_eq!(plan.next_inventory.items().count(), BAG_SLOTS as usize);
        assert_eq!(plan.next_progression.gold, 95);
        let replay = plan.items.iter().fold(bag.clone(), |b,c| b.apply(&inventory::Command::decode(&c.encode()).unwrap()).unwrap());
        assert_eq!(replay, plan.next_inventory);
        assert!(plan_buy(stock, &plan.next_inventory, &plan.next_progression, new.id+1).is_err());
        assert_eq!(plan.next_progression.gold, 95);
        assert_eq!(bag.items().find(|i|i.id==silver.id).unwrap().quantity, 65_400);
        assert_eq!(gold.gold, 100);
        let no_gold = Progression::default();
        assert!(plan_buy(stock, &bag, &no_gold, bag.high_water()+1).is_err());
    }
}
