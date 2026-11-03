//! Story scenes: the Quest panel's Story page (`gmStoryQuestsUI`) lists a scene as playable once
//! the player's `ActRegistry` holds it.
//!
//! Retail (client, 7230 wlib): the server calls `WM_Quest` 0x23 -> `CPlayer` 0x2A ->
//! `CQuestSystem::UpdateStoryQuest_Done(errorCode, bAdd, sceneId)` (stack order -5, -4, -3; the
//! script branches on -4 for add/remove and passes -3 to `ActRegistry::AddSceneID`), which
//! adds or removes the scene in the player's registry and raises
//! `CM_Quest::SendNotice_UpdateStoryQuests`. The login's registry (`WM_Player` 0x27 argument 2) is
//! sent empty, as for effects, and each scene still earned is granted again after it.
//!
//! What triggered a grant on the retail server is not in the client or the DATs (`Eff_BestowScene*`
//! effects carry a scene id; nothing in the wlib or AC2Hero says when the server applied them).
//! Pyrealium policy: a character that has earned no scene is granted the three scenes of the Prologue
//! act (`ActPrologue`, act 0x40000008) when it first enters the world.
//!
//! The client reports a played scene with `WM_Quest` func 0x22 (0x82130013, `HandlePlayScenesDone`,
//! a packed `PList<gmSceneInfo>`). Nothing a retail server did with it is known; Pyrealium accepts it.
use crate::{script, world::WorldError};

/// `WM_Quest` 0x23: the server's grant (or removal) of a scene.
pub const GRANT: u32 = 0x82130023;
/// `WM_Quest` 0x22: the client's played-scene list.
pub const PLAYED: u32 = 0x82130013;
/// Policy: the Prologue act's scenes (act 0x40000008), in scene-number order.
pub const PROLOGUE: [u32; 3] = [0x4000_0021, 0x4000_0067, 0x4000_0068];

/// Scenes the server can grant: the Prologue's (the only ones this policy gives).
pub fn is_scene(id: u32) -> bool {
    PROLOGUE.contains(&id)
}

/// `UpdateStoryQuest_Done(0, 1, scene)`: add `scene` to the player's registry.
pub fn granted(scene: u32) -> Result<Vec<u8>, WorldError> {
    let arguments = [script::integer(0), script::integer(1), script::integer(scene)].concat();
    script::private_call(GRANT, &arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quests::{Book, Command};

    #[test]
    fn a_scene_is_earned_once_and_survives_the_journal() {
        let book = Book::default().apply(&Command::Scene(PROLOGUE[0])).unwrap();
        assert_eq!(book.scenes().collect::<Vec<_>>(), [PROLOGUE[0]]);
        assert_eq!(book.apply(&Command::Scene(PROLOGUE[0])).unwrap(), book);
        let text = Command::Scene(PROLOGUE[1]).encode();
        assert_eq!(text, "scene,40000067");
        assert_eq!(Command::decode(&text).unwrap(), Command::Scene(PROLOGUE[1]));
        assert!(Book::default().apply(&Command::Scene(1)).is_err());
    }

    #[test]
    fn the_grant_is_the_scripts_add_call() {
        let call = granted(0x4000_0021).unwrap();
        let arguments = [script::integer(0), script::integer(1), script::integer(0x4000_0021)].concat();
        assert!(call.windows(arguments.len()).any(|w| w == arguments.as_slice()));
        assert!(call.windows(4).any(|w| w == GRANT.to_le_bytes()));
    }
}
