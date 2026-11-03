//! Read service identity from installed EntityDesc properties, never display names.
//! EntityDesc::UnPack / MasterProperty 34000000; DoorTemplate's Legions classes
//! 3756..3764 and 3922 derive from Door. Stock is separately authored server policy.
use crate::dat::Dat;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Door { locked: bool },
    Chest { locked: bool },
    Sign,
    Vendor,
    /// A prospected resource deposit (tradeskills.rs); only a live scene object has it.
    Deposit,
}
pub struct Profiles {
    types: BTreeMap<u32, u32>,
    cache: BTreeMap<u32, Option<Service>>,
}
fn invalid() -> io::Error {
    io::Error::other("unsupported service EntityDesc")
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(invalid)?;
        let b = self.bytes.get(self.pos..end).ok_or_else(invalid)?;
        self.pos = end;
        Ok(b)
    }
    fn word(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn align(&mut self) -> io::Result<()> {
        self.take((4 - self.pos % 4) % 4)?;
        Ok(())
    }
    fn string(&mut self, width: usize) -> io::Result<()> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().unwrap());
        let n = if n == 65535 {
            self.word()? as usize
        } else {
            n as usize
        };
        if n > 65536 {
            return Err(invalid());
        }
        self.take(n * width)?;
        self.align()
    }
    fn count(&mut self) -> io::Result<usize> {
        let n = self.word()? as usize;
        if n > 65536 { Err(invalid()) } else { Ok(n) }
    }
}
impl Profiles {
    pub fn open(dat: &mut Dat) -> io::Result<Self> {
        let bytes = dat.read(0x34000000)?;
        let mut r = Reader {
            bytes: &bytes,
            pos: 0,
        };
        if r.word()? != 0x34000000 {
            return Err(invalid());
        }
        r.take(8)?;
        for _ in 0..r.count()? {
            r.word()?;
            r.string(1)?;
        }
        let mut types = BTreeMap::new();
        let count = r.word()? & 65535;
        for _ in 0..count {
            let key = r.word()?;
            types.insert(key, r.word()? & 65535);
            r.take(16)?;
        }
        Ok(Self {
            types,
            cache: BTreeMap::new(),
        })
    }
    pub fn get(&mut self, dat: &mut Dat, did: u32) -> Option<Service> {
        if let Some(value) = self.cache.get(&did) {
            return *value;
        }
        let result = self.resolve(dat, did).ok().flatten();
        self.cache.insert(did, result);
        result
    }
    fn resolve(&self, dat: &mut Dat, mut did: u32) -> io::Result<Option<Service>> {
        let mut props = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for _ in 0..32 {
            if did >> 24 != 0x47 {
                return Ok(if (3756..=3764).contains(&did) || did == 3922 {
                    Some(Service::Door {
                        locked: [0x41000195, 0x410000b6, 0x41000133, 0x41000131, 0x41000135]
                            .iter()
                            .any(|k| props.get(k).is_some_and(|v| *v != 0)),
                    })
                } else if did == 3723 {
                    // Chest templates (Drudge Potion, Encomium and Supply chests).
                    Some(Service::Chest {locked:[0x41000195,0x410000b6,0x41000133,0x41000131,0x41000135]
                        .iter().any(|k|props.get(k).is_some_and(|v|*v!=0))})
                } else if did == 411 || (4000..=4075).contains(&did) {
                    // WLib RoadSignTemplate and its town/gateway subclasses.
                    Some(Service::Sign)
                } else if [0x410002de, 0x410002ec]
                    .iter()
                    .any(|k| props.get(k).is_some_and(|v| *v != 0))
                {
                    Some(Service::Vendor)
                } else {
                    None
                });
            }
            if !seen.insert(did) {
                return Err(invalid());
            }
            let bytes = dat.read(did)?;
            let mut r = Reader {
                bytes: &bytes,
                pos: 0,
            };
            let flags = r.word()?;
            if flags & !511 != 0 {
                return Err(invalid());
            }
            if flags & 1 != 0 {
                r.word()?;
            }
            if flags & 2 != 0 {
                r.word()?;
            }
            if flags & 4 != 0 {
                r.take(8)?;
            }
            did = if flags & 8 != 0 { r.word()? } else { 0 };
            if flags & 16 != 0 {
                r.take(64)?;
            }
            if flags & 32 != 0 {
                r.take(12)?;
            }
            if flags & 128 != 0 {
                for _ in 0..r.count()? {
                    r.word()?;
                    for _ in 0..r.count()? {
                        let key = r.word()?;
                        let kind = *self.types.get(&key).ok_or_else(invalid)?;
                        match kind {
                            1 | 2 | 3 | 5 | 7 | 8 | 11 => {
                                let value = r.word()?;
                                props.entry(key).or_insert(value);
                            }
                            6 => r.string(1)?,
                            4 => {
                                r.take(12)?;
                            }
                            9 => {
                                r.take(44)?;
                            }
                            13 => {
                                r.take(8)?;
                            }
                            14 => {
                                r.take(32)?;
                            }
                            10 => {
                                r.take(8)?;
                                let n = r.word()?;
                                if n & 65535 != 0 {
                                    return Err(invalid());
                                }
                                if n >> 16 != 0 {
                                    r.string(2)?;
                                }
                            }
                            _ => return Err(invalid()),
                        }
                    }
                }
            }
            if flags & 64 != 0 {
                r.word()?;
            }
            if flags & 256 != 0 {
                r.string(1)?;
            }
            if r.pos != bytes.len() {
                return Err(invalid());
            }
        }
        Err(invalid())
    }
}

pub const OPEN_MODE: u32 = 0x41000002;
pub const CLOSED_MODE: u32 = 0x41000003;
/// Native DoMode (00495420), cached alongside the spawn for late observers.
pub fn door_mode(id: u64, instance: u16, stamp: u16, open: bool) -> Vec<u8> {
    let mut bytes = crate::combat_visual::DO_MODE.to_le_bytes().to_vec();
    bytes.extend(id.to_le_bytes());
    bytes.extend(instance.to_le_bytes());
    bytes.extend(stamp.to_le_bytes());
    bytes.extend(if open { OPEN_MODE } else { CLOSED_MODE }.to_le_bytes());
    bytes
}
