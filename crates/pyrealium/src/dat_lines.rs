// SPDX-License-Identifier: AGPL-3.0-only
//! The DAT string-table lines an author picks from (`GET /admin/strings`): an NPC's own table
//! (from its EntityDesc name) or any table, each key with its rendered text. Quest rows speak
//! them by reference (`Tell table key`).
use crate::{dat::Dat, examination::{Name, Names}};
use std::{io, path::Path};

pub struct Lines { portal: Dat, tables: ac2_db::StoreStringTables }

impl Lines {
    pub fn open(game_data: &Path) -> io::Result<Self> {
        let dataset = ac2_dat::DatDataset::open(ac2_dat::DatOpenOptions::for_retail_install(game_data)).map_err(io::Error::other)?;
        Ok(Self { portal: Dat::open(game_data.join("portal.dat"))?,
            tables: ac2_db::StoreStringTables::new(ac2_db::ResourceStore::new(dataset), 0x5800_0000) })
    }
    /// The string table an entity's name comes from (an NPC's own lines).
    pub fn table_of(&mut self, entity: u32) -> Option<u32> {
        match Names::read(&mut self.portal, entity).ok()? { Name::Localized([_, table, _]) => Some(table), _ => None }
    }
    /// Every line of `table`, by key, with its text (meta tags kept, as authors see them).
    pub fn lines(&self, table: u32) -> io::Result<Vec<(u32, String)>> {
        if table >> 24 != 0x25 { return Err(io::Error::other("a string table DID is 0x25......")); }
        let loaded = ac2_db::load_string_table(self.tables.store(), ac2_core::DataId::new(table)).map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("no such string table"))?;
        let mut keys: Vec<u32> = loaded.strings.keys().copied().collect();
        keys.sort_unstable();
        Ok(keys.into_iter().map(|key| {
            let info = ac2_db::RenderInfo { string_id: key, table_id: table, ..Default::default() };
            let text = ac2_db::inq_string(&self.tables, &info, true).map(|t| String::from_utf16_lossy(&t)).unwrap_or_default();
            (key, text)
        }).collect())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_npc_s_own_table_and_its_lines() {
        let Ok(data) = std::env::var("AC2_TEST_GAME_DATA") else { return };
        let mut lines = super::Lines::open(std::path::Path::new(&data)).unwrap();
        // Moira Len (wsl_kq2g_moira) speaks from kq2g_moira.stt.
        assert_eq!(lines.table_of(0x4700_3fae), Some(0x2500_895c));
        let found = lines.lines(0x2500_895c).unwrap();
        assert!(found.iter().any(|(k, t)| *k == 0x0920_0efe && t == "Sorry, I don't have a task for you today."), "{found:?}");
        assert!(lines.lines(0x4700_3fae).is_err());
    }
}
