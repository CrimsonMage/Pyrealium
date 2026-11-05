//! The prepare step's tests (`content_prep`).
use super::*;

fn sha(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}
fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("pyrl-prep-{}-{}", std::process::id(), crate::auth::hex(&crate::auth::random::<6>().unwrap())));
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn write(content: &Path, name: &str, rows: &[&str], header: &str, trailing: bool) {
    let mut text = format!("{header}\n{}", rows.join("\n"));
    if trailing {
        text.push('\n');
    }
    std::fs::write(content.join(name), text).unwrap();
}
fn index(portal: &[u32], cell: &[u32]) -> AssetIndex {
    AssetIndex { portal: portal.iter().copied().collect(), cell: cell.iter().copied().collect() }
}
const FULL: Options = Options { training_placements: true, shelter_quest: true };
/// Prepares the fixture from its files, and again from the same packs as content-database
/// rows: both give the same world and report (the move into the database changes nothing).
fn run(index: &AssetIndex, visual: u32, content: &Path) -> io::Result<Prepared> {
    let files = prepare_with(index, &mut |_| Some(visual), "dat", &Source::Dir(content), FULL);
    let mut db = crate::content_db::empty().unwrap();
    if let (Ok(files), Ok(_), Ok(_)) = (&files, crate::content_db::world::import_packs(&mut db, content),
        crate::content_db::files::import(&mut db, content)) {
        let rows = prepare_with(index, &mut |_| Some(visual), "dat", &Source::Db { db, dir: content }, FULL).unwrap();
        assert_eq!((&rows.world, &rows.report), (&files.world, &files.report), "the database gives the files' world");
    }
    files
}
fn world_lines(p: &Prepared) -> Vec<String> {
    p.world.lines().map(str::to_owned).collect()
}
/// The base files every fixture needs, plus the four quest tables the preparation copies.
fn base(content: &Path, header: &str) {
    for name in ["world-observed-expanded.tsv", "training-world.tsv", "capture-spawns.tsv"] {
        write(content, name, &[], header, false);
    }
    for name in TABLES {
        std::fs::write(content.join(name), "").unwrap();
    }
}

#[test]
fn evidence_hash_accepts_checkout_line_endings_without_accepting_edits() {
    let d = dir();
    let path = d.join("content.tsv");
    let lf = b"header\nentry\tvalue\n".to_vec();
    let crlf = String::from_utf8(lf.clone()).unwrap().replace('\n', "\r\n").into_bytes();
    let expected = [sha(&lf), sha(&crlf)];
    for data in [&lf, &crlf] {
        std::fs::write(&path, data).unwrap();
        assert_eq!(sha256_any_eol(&path).unwrap(), expected);
    }
    std::fs::write(&path, b"header\nentry\tchanged\n").unwrap();
    assert!(sha256_any_eol(&path).unwrap().iter().all(|d| !expected.contains(d)));
    std::fs::remove_dir_all(d).unwrap();
}

fn authored_pack_evidence(pack: &str, version: u32, catalog_name: Option<&str>) {
    let content = dir();
    base(&content, "pyrealium-world-v4");
    let row = "8140c0000077b800\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t-\tCreature";
    let manifest = content.join(format!("{pack}-world-v{version}.tsv"));
    std::fs::write(&manifest, format!("pyrealium-world-v4\n{row}\n")).unwrap();
    let catalog = content.join(catalog_name.map_or_else(|| format!("{pack}-content-v{version}.json"), str::to_owned));
    std::fs::write(&catalog, "{}").unwrap();
    let evidence = json!({"native_ground_verified": true,
        "manifest_sha256": sha(&std::fs::read(&manifest).unwrap()),
        "source_sha256": sha(&std::fs::read(&catalog).unwrap())});
    std::fs::write(manifest.with_extension("report.json"), evidence.to_string()).unwrap();
    let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff]);
    let out = run(&idx, 0x1f000001, &content).unwrap();
    assert_eq!(world_lines(&out), ["pyrealium-world-v4", row]);
    let out = run(&idx, 0x1f000002, &content).unwrap();
    assert_eq!(world_lines(&out).len(), 1, "a visual the DATs disagree with is withheld");
    std::fs::write(&catalog, "{\"changed\":true}").unwrap();
    let error = run(&idx, 0x1f000001, &content).err().unwrap();
    assert!(error.to_string().contains("fresh native ground"), "{error}");
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn authored_packs_require_matching_ground_proof_and_native_visual() {
    authored_pack_evidence("regional", 1, None);
    authored_pack_evidence("exploration", 1, None);
    authored_pack_evidence("hunting", 2, None);
    authored_pack_evidence("npc", 1, Some("npc-dialogue-v1.json"));
}

const OLD: &str = "8140000000000001\t47001c27\t1f001042\t76b80027\t98.1885986\t133.502838\t168.323868\t1\t0\t0\t0\t-";
const NEW: &str = "8140000000000002\t47001c27\t1f001042\t77b80006\t1.37962341\t100.779083\t182.50499\t1\t0\t0\t0\t-\tOrven";
fn replacement_fixture() -> (PathBuf, AssetIndex, String) {
    let content = dir();
    base(&content, "pyrealium-world-v4");
    // The same template in another location is not automatically a duplicate.
    let other = OLD.replace("8140000000000001", "8140000000000003").replace("76b80027", "80b80027");
    write(&content, "world-observed-expanded.tsv", &[OLD, &other], "pyrealium-world-v4", true);
    write(&content, "training-world.tsv", &[], "pyrealium-world-v4", true);
    write(&content, "capture-spawns.tsv", &[], "pyrealium-world-v4", true);
    write(&content, "recovered-population.tsv", &[NEW], "pyrealium-world-v4", true);
    let recovered = content.join("recovered-population.tsv");
    std::fs::write(recovered.with_extension("report.json"),
        json!({"native_ground_verified": true, "manifest_sha256": sha(&std::fs::read(&recovered).unwrap())}).to_string()).unwrap();
    let endpoint = |source: &str, row: &str| json!({"source": source, "id": first(row), "row_sha256": sha(row.as_bytes())});
    let policy = json!({"schema": 1, "replacements": [{"name": "Orven", "reason": "Audited duplicate",
        "superseded": endpoint("world-observed-expanded.tsv", OLD),
        "retained": endpoint("recovered-population.tsv", NEW)}]});
    std::fs::write(content.join("placement-replacements.json"), policy.to_string()).unwrap();
    (content, index(&[0x47001c27, 0x1f001042], &[0x76b8ffff, 0x77b8ffff, 0x80b8ffff]), other)
}
#[test]
fn audited_npc_replacement_crosses_cells_without_template_deduplication() {
    let (content, idx, other) = replacement_fixture();
    let out = run(&idx, 0x1f001042, &content).unwrap();
    assert_eq!(world_lines(&out), ["pyrealium-world-v4".to_owned(), format!("{other}\t-"), NEW.to_owned()]);
    assert_eq!(out.report["included"], 2);
    assert_eq!(out.report["excluded"], 1);
    assert_eq!(out.report["superseded_placements"][0]["superseded"]["id"], first(OLD));
    assert!(out.report["unapplied_placement_replacements"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn unavailable_replacement_preserves_original_npc() {
    let (content, mut idx, other) = replacement_fixture();
    idx.cell.remove(&0x77b8ffff);
    let out = run(&idx, 0, &content).unwrap();
    assert_eq!(world_lines(&out), ["pyrealium-world-v4".to_owned(), format!("{OLD}\t-"), format!("{other}\t-")]);
    assert!(out.report["superseded_placements"].as_array().unwrap().is_empty());
    assert_eq!(out.report["unapplied_placement_replacements"].as_array().unwrap().len(), 1);
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn stale_replacement_proof_is_refused() {
    for endpoint in ["superseded", "retained"] {
        let (content, idx, _) = replacement_fixture();
        let path = content.join("placement-replacements.json");
        let mut policy: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        policy["replacements"][0][endpoint]["row_sha256"] = json!("stale");
        std::fs::write(&path, policy.to_string()).unwrap();
        let error = run(&idx, 0x1f001042, &content).err().unwrap();
        assert!(error.to_string().contains("Stale placement replacement evidence"), "{endpoint}: {error}");
        std::fs::remove_dir_all(content).unwrap();
    }
}
#[test]
fn verified_translation_wins_over_old_cell_snapshots() {
    let content = dir();
    base(&content, "pyrealium-world-v4");
    let row = "8140000000000001\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t-\tBeetle";
    let old = row.replace("00000001\t470", "00000002\t470");
    write(&content, "capture-spawns.tsv", &[&old], "pyrealium-world-v4", true);
    write(&content, "recovered-population.tsv", &[row], "pyrealium-world-v4", true);
    let path = content.join("recovered-population.tsv");
    std::fs::write(path.with_extension("report.json"),
        json!({"native_ground_verified": true, "manifest_sha256": sha(&std::fs::read(&path).unwrap())}).to_string()).unwrap();
    let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff]);
    assert_eq!(world_lines(&run(&idx, 0x1f000001, &content).unwrap()), ["pyrealium-world-v4", row]);
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("5\t5\t0", "6\t5\t0")).unwrap();
    assert!(run(&idx, 0x1f000001, &content).err().unwrap().to_string().contains("ground verification"));
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn audited_pack_respects_upstream_cells_and_exact_state_evidence() {
    let content = dir();
    base(&content, "pyrealium-world-v4");
    let row = "8140000000000001\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t-\t-";
    let changed = row.replace("00000001\t470", "00000002\t470").replace("01020001", "01030001");
    let overlap = row.replace("00000001\t470", "00000003\t470");
    let fresh = row.replace("00000001\t470", "00000004\t470").replace("01020001", "01040001");
    write(&content, "capture-spawns-upstream.tsv", &[row, &changed], "pyrealium-world-v4", false);
    write(&content, "capture-spawns-audited.tsv", &[&overlap, &fresh], "pyrealium-world-v4", false);
    let proof = json!({"excluded_existing": [
        {"id": first(row), "source": "capture-spawns-upstream.tsv", "row_sha256": sha(row.as_bytes())},
        {"id": first(&changed), "source": "capture-spawns-upstream.tsv", "row_sha256": "outdated-evidence"}]});
    std::fs::write(content.join("capture-spawns-audited.report.json"), proof.to_string()).unwrap();
    let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff, 0x0103ffff, 0x0104ffff]);
    let out = run(&idx, 0x1f000001, &content).unwrap();
    assert_eq!(world_lines(&out), ["pyrealium-world-v4", &changed, &fresh]);
    assert_eq!(out.report["transient_capture_placements"].as_array().unwrap().len(), 1);
    assert_eq!(out.report["overlapping_capture_cells"].as_array().unwrap().len(), 1);
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn door_defaults_respect_nested_strings_and_truncated_tables() {
    let words = |v: &[u32]| v.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
    // Display name contains a tagged nested StringInfo, followed by a DID table.
    let mut name = words(&[0, 0, 1, 0x41000001]);
    name.extend(9u16.to_le_bytes());
    name.extend(0u16.to_le_bytes());
    name.extend(words(&[0, 0, 0]));
    let mut data = words(&[0x21, 0x8100114a, 4]);
    data.extend(name);
    data.extend(words(&[1, 1, 0x47001145]));
    assert_eq!(door_physical(&data).unwrap(), Some(0x47001145));
    for end in 0..data.len() {
        assert!(door_physical(&data[..end]).is_err(), "accepted a table cut at {end}");
    }
}
#[test]
fn existing_did_with_wrong_model_is_withheld_not_repositioned() {
    let content = dir();
    base(&content, "pyrealium-world-v3");
    let wrong = "8140aab372ef1689\t4700334c\t1f0010a8\t77b80006\t1.37962341\t100.779083\t182.50499\t1\t0\t0\t0\t-";
    let valid = wrong.replace("8140aab372ef1689", "8140000000000002").replace("1f0010a8", "1f0010a6");
    write(&content, "capture-spawns.tsv", &[wrong, &valid], "pyrealium-world-v3", false);
    let idx = index(&[0x4700334c, 0x1f0010a8, 0x1f0010a6], &[0x77b8ffff]);
    let out = run(&idx, 0x1f0010a6, &content).unwrap();
    assert_eq!(world_lines(&out), ["pyrealium-world-v3", &valid]);
    assert_eq!(out.report["incompatible_capture_placements"][0]["id"], "8140aab372ef1689");
    assert_eq!(out.report["incompatible_capture_placements"][0]["dat_visual"], "1f0010a6");
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn landmark_comparison_crosses_outdoor_cells_but_not_dungeon_rooms() {
    assert!(nearby_landmark(0x76b80022, [85.5575, 29.5398, 182.505], 0x76b8001a, [77.260666, 27.62573, 182.5]));
    assert!(nearby_landmark(0x76b80040, [159.0, 20.0, 182.5], 0x77b80001, [1.0, 20.0, 182.5]));
    assert!(!nearby_landmark(0x02990109, [0.0, 0.0, -44.0], 0x0299010a, [0.0, 0.0, -44.0]));
    assert!(!nearby_landmark(0x76b80022, [0.0, 0.0, 182.0], 0x76b80022, [13.0, 0.0, 182.0]));
}
#[test]
fn new_capture_names_preserved_without_stacking_cell_snapshots() {
    let content = dir();
    base(&content, "pyrealium-world-v4");
    let old = "8140000000000001\t47000001\t1f000001\t01020001\t5\t5\t0\t1\t0\t0\t0\t1";
    let overlap = format!("{}\tNew observation", old.replace("00000001\t470", "00000002\t470"));
    let fresh = format!("{}\tNamed gateway", old.replace("00000001\t470", "00000003\t470").replace("01020001", "01030001"));
    write(&content, "capture-spawns.tsv", &[old], "pyrealium-world-v4", false);
    write(&content, "capture-spawns-upstream.tsv", &[&overlap, &fresh], "pyrealium-world-v4", false);
    let idx = index(&[0x47000001, 0x1f000001], &[0x0102ffff, 0x0103ffff]);
    let out = run(&idx, 0x1f000001, &content).unwrap();
    assert_eq!(world_lines(&out), ["pyrealium-world-v4".to_owned(), format!("{old}\t-"), fresh]);
    assert_eq!(out.report["overlapping_capture_cells"].as_array().unwrap().len(), 1);
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn stripped_training_placements_leave_the_mirrored_shelter_and_its_quest() {
    let content = dir();
    std::fs::write(content.join("training-world.tsv"),
        "pyrealium-world-v3\n# kit\n8140029900000001\t47000001\t1f000001\t0299011a\n").unwrap();
    std::fs::write(content.join("human-shelter-world.tsv"),
        "pyrealium-world-v4\n# evidence\n8140029900000100\t47000002\t1f000002\t0299014d\n").unwrap();
    let mut portal: Vec<String> = ["8140abcd12345678", "47001451", "1f000003", "02980120"].map(str::to_owned).to_vec();
    portal.extend(std::iter::repeat_n("0".to_owned(), 8));
    portal.extend(["-".to_owned(), "Portal".to_owned()]);
    let lines: Vec<String> = vec![
        "pyrealium-world-v4".into(),
        "8140029900000001\t47000001\t1f000001\t0299011a".into(),
        portal.join("\t"),
        "8140029912345678\t47000004\t1f000004\t01020001\t5".into(),
    ];
    let (result, counts) = without_training_placements(&Source::Dir(&content), lines.clone(), true).unwrap();
    let mut mirrored: Vec<String> = ["8140029912345679", "47001451", "1f000003", "02990120"].map(str::to_owned).to_vec();
    mirrored.extend(std::iter::repeat_n("0".to_owned(), 8));
    mirrored.extend([QUICK_EXIT_NAME.to_owned(), "Portal".to_owned()]);
    let mut expected = vec![lines[0].clone(), lines[2].clone(), lines[3].clone()];
    expected.push(mirrored.join("\t"));
    expected.push("8140029900000100\t47000002\t1f000002\t0299014d".into());
    assert_eq!(result, expected);
    assert_eq!(counts, json!({"training_placements_removed": 1, "shelter_mirrored": 1, "shelter_quest_placements": 1}));
    let (result, counts) = without_training_placements(&Source::Dir(&content), lines, false).unwrap();
    assert_eq!(result.len(), 4);
    assert_eq!(counts["shelter_quest_placements"], 0);
    std::fs::remove_dir_all(content).unwrap();
}
#[test]
fn readers_reject_overruns() {
    assert!(door_physical(&[1, 0, 0, 0]).is_err());
    let mut r = Reader::new(&[3, 0, b'a', b'b', b'c', 0]);
    assert!(r.string(1).is_err(), "aligned extent passes the end");
    let mut r = Reader::new(&[2, 0, b'a', b'b']);
    assert!(r.string(1).is_ok() && r.pos == 4);
}
/// The 7230 visual resolution, checked against the installed DATs when present (Legions-era
/// expectations of the Python suite did not carry over; 7230 is the target data).
#[test]
fn installed_7230_templates_resolve_to_visuals() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../dataset-7230");
    if !directory.join("portal.dat").exists() {
        eprintln!("skipped: no dataset-7230 beside the workspace");
        return;
    }
    let mut profile = ContentProfile::open(&directory).unwrap();
    assert_eq!(profile.visual(0x470007de), Some(0x1f00002d), "lifestone, as the prepared world places it");
}

mod database_parity {
    /// The shipped content prepared from the database (its `world_*` rows and the packs' catalog
    /// documents) is the world the pack files prepare (needs the 7230 DATs: AC2_TEST_GAME_DATA).
    #[test]
    fn the_shipped_world_from_the_database_is_the_files_world() {
        let Ok(data) = std::env::var("AC2_TEST_GAME_DATA") else { return };
        // The shipped content as files (checked out of the dump) against the dump itself.
        let content = crate::content_db::checkout::repository().to_path_buf();
        // The shipped dump, as the server reads it.
        let shipped = std::fs::read_to_string(content.join(crate::content_db::DUMP)).unwrap();
        for options in [super::Options { training_placements: true, shelter_quest: true },
                        super::Options { training_placements: false, shelter_quest: true }] {
            let index = super::asset_index(std::path::Path::new(&data)).unwrap();
            let mut profile = super::ContentProfile::open(std::path::Path::new(&data)).unwrap();
            let files = super::prepare_with(&index, &mut |d| profile.visual(d), "dat", &super::Source::Dir(&content), options).unwrap();
            let rows = crate::content_db::open_dump(&shipped).unwrap();
            let from_db = super::prepare_with(&index, &mut |d| profile.visual(d), "dat", &super::Source::Db { db: rows, dir: &content }, options).unwrap();
            assert_eq!(from_db.world, files.world);
            assert_eq!(from_db.report, files.report);
        }
    }
}
