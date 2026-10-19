//! The clean-up steps one at a time, on small hand-made DATs, and the whole pipeline on text
//! DATs. Expected values are worked out from the C# (`DAT/DATReader/DatClean/*.cs`) by hand.

use rv_core::{
    DatStatus, FileType, FilterType, HeaderFileType, HeaderType, MergeType, MiaStatus,
    RemoveSubType, ZipStructure,
};

use super::*;
use crate::{DatFile, DatGame};

fn rom(name: &str, size: u64, crc: u32) -> DatFile {
    let mut f = DatFile::new(name);
    f.size = Some(size);
    f.crc = Some(crc.to_be_bytes().to_vec());
    f
}

fn game_with(name: &str, info: DatGame, roms: Vec<DatFile>) -> DatNode {
    let mut d = DatDir::new(name, FileType::UNSET);
    d.game = Some(Box::new(info));
    for r in roms {
        d.child_add(DatNode::File(r));
    }
    DatNode::Dir(d)
}

fn game(name: &str, roms: Vec<DatFile>) -> DatNode {
    game_with(name, DatGame::default(), roms)
}

fn clone_game(name: &str, parent: &str, roms: Vec<DatFile>) -> DatNode {
    game_with(
        name,
        DatGame {
            clone_of: parent.to_owned(),
            rom_of: parent.to_owned(),
            ..DatGame::default()
        },
        roms,
    )
}

fn root(children: Vec<DatNode>) -> DatDir {
    let mut d = DatDir::new("", FileType::UNSET);
    for c in children {
        d.child_add(c);
    }
    d
}

fn names(d: &DatDir) -> Vec<&str> {
    d.children.iter().map(DatNode::name).collect()
}

fn dir<'a>(d: &'a DatDir, name: &str) -> &'a DatDir {
    d.children
        .iter()
        .find_map(|c| match c {
            DatNode::Dir(x) if x.base.name == name => Some(x),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no dir {name} in {:?}", names(d)))
}

fn file<'a>(d: &'a DatDir, name: &str) -> &'a DatFile {
    d.children
        .iter()
        .find_map(|c| match c {
            DatNode::File(x) if x.base.name == name => Some(x),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no file {name} in {:?}", names(d)))
}

fn rule() -> CleanRule {
    CleanRule {
        filter: FilterType::KeepAll,
        use_id_for_name: false,
        add_category_sub_dirs: false,
        category_order: Vec::new(),
        merge: MergeType::None,
        merge_override_dat: false,
        compression: FileType::ZIP,
        compression_override_dat: false,
        compression_sub: ZipStructure::ZIP_TRRNT,
        convert_while_fixing: true,
        single_archive: false,
        sub_dir_type: RemoveSubType::KeepAllSubDirs,
        use_description_as_dir_name: false,
        header_type: HeaderType::Optional,
    }
}

// ---- steps 1 to 5, 7, 8 ---------------------------------------------------------------------

#[test]
fn step1_names_get_forward_slashes_and_lose_forbidden_characters() {
    let mut r = root(vec![game(
        "Dir\\Game: One?",
        vec![rom("a\\b./c*d<e>f|g\"h", 1, 1), rom("tab\there", 1, 2)],
    )]);
    clean_filenames(&mut r);
    let g = dir(&r, "Dir/Game- One-");
    assert_eq!(names(g), ["a/b/c-d-e-f-g-h", "tab-here"]);
}

#[test]
fn step2_filters_keep_roms_or_disks() {
    let mut disk = DatFile::new("d");
    disk.is_disk = true;
    let build = || root(vec![game("g", vec![rom("r", 1, 1), disk.clone()])]);
    let mut r = build();
    remove_chd(&mut r).unwrap();
    assert_eq!(names(dir(&r, "g")), ["r"]);
    let mut r = build();
    remove_non_chd(&mut r).unwrap();
    assert_eq!(names(dir(&r, "g")), ["d"]);
}

#[test]
fn a_loose_file_where_a_game_should_be_is_an_error_not_a_panic() {
    let mut r = root(vec![DatNode::File(rom("loose", 1, 1))]);
    assert!(remove_chd(&mut r).is_err());
    assert!(remove_non_chd(&mut r).is_err());
    assert!(make_merge_set(&mut r, false).is_err());
    assert!(set_match_ids(&mut r).is_err());
}

#[test]
fn step3_nodump_roms_are_dropped_inside_games_and_nested_dirs() {
    let mut nd = rom("bad", 1, 0);
    nd.status = "nodump".into();
    let mut sub = DatDir::new("sub", FileType::UNSET);
    sub.child_add(game("g2", vec![nd.clone(), rom("ok", 2, 2)]));
    let mut r = root(vec![
        game("g1", vec![nd, rom("keep", 1, 1)]),
        DatNode::Dir(sub),
    ]);
    remove_no_dumps(&mut r);
    assert_eq!(names(dir(&r, "g1")), ["keep"]);
    assert_eq!(names(dir(dir(&r, "sub"), "g2")), ["ok"]);
}

#[test]
fn step4_ids_prefix_every_item_and_leak_to_later_siblings() {
    let with_id = |id: &str| DatGame {
        id: id.to_owned(),
        ..DatGame::default()
    };
    let mut r = root(vec![
        game_with("A", with_id("0001"), vec![rom("a.rom", 1, 1)]),
        game("B", vec![rom("b.rom", 1, 2)]),
        game_with("C", with_id("0003"), vec![rom("c.rom", 1, 3)]),
    ]);
    set_add_id_numbers(&mut r, "");
    // B has no id of its own and takes the one A set; the ROM files are renamed too.
    assert_eq!(names(&r), ["0001 - A", "0001 - B", "0003 - C"]);
    assert_eq!(names(dir(&r, "0001 - B")), ["0001 - b.rom"]);
}

#[test]
fn step5_category_single_ranked_or_none() {
    let cats = |c: &[&str]| DatGame {
        category: c.iter().map(|s| (*s).to_owned()).collect(),
        ..DatGame::default()
    };
    let order = vec!["Action".to_owned(), "Puzzle".to_owned()];
    let one = match game_with("g", cats(&["Racing"]), vec![]) {
        DatNode::Dir(d) => d,
        _ => unreachable!(),
    };
    assert_eq!(find_category(&one, &order).as_deref(), Some("Racing"));
    let two = match game_with("g", cats(&["puzzle", "ACTION", "Other"]), vec![]) {
        DatNode::Dir(d) => d,
        _ => unreachable!(),
    };
    assert_eq!(find_category(&two, &order).as_deref(), Some("Action"));
    let none = match game_with("g", cats(&["x", "y"]), vec![]) {
        DatNode::Dir(d) => d,
        _ => unreachable!(),
    };
    assert_eq!(find_category(&none, &order), None);
    let mut r = root(vec![
        game_with("g", cats(&["Racing"]), vec![rom("r", 1, 1)]),
        game("h", vec![]),
    ]);
    add_category(&mut r, &order);
    assert_eq!(names(&r), ["Racing/g", "h"]);
}

#[test]
fn step7_status_mia_merge_prefix_and_nodump() {
    let mut mia = rom("mia", 5, 1);
    mia.mia = "YES".into();
    let mut mia_zero = rom("mia0", 0, 2);
    mia_zero.mia = "yes".into();
    let mut mia_nosize = DatFile::new("mia_nosize");
    mia_nosize.mia = "yes".into();
    let mut merged = rom("merged", 1, 3);
    merged.base.dat_status = DatStatus::IN_DAT_MERGED;
    merged.merge = "parent.rom".into();
    let mut with_merge = rom("wm", 1, 4);
    with_merge.merge = "m.rom".into();
    let mut nd = rom("nd", 1, 5);
    nd.status = "nodump".into();
    let mut r = root(vec![game(
        "g",
        vec![mia, mia_zero, mia_nosize, merged, with_merge, nd],
    )]);
    set_status(&mut r);
    let g = dir(&r, "g");
    assert_eq!(file(g, "mia").mia_status, MiaStatus::MIA_FROM_DAT);
    assert_eq!(
        file(g, "mia0").mia_status,
        MiaStatus::NONE,
        "size 0 is not MIA"
    );
    assert_eq!(
        file(g, "mia_nosize").mia_status,
        MiaStatus::MIA_FROM_DAT,
        "`Size != 0` is true for a missing size"
    );
    // Already merged: untouched, no prefix.
    assert_eq!(file(g, "merged").base.dat_status, DatStatus::IN_DAT_MERGED);
    assert_eq!(file(g, "merged").merge, "parent.rom");
    assert_eq!(file(g, "wm").merge, "(No-Merge) m.rom");
    assert_eq!(file(g, "wm").base.dat_status, DatStatus::IN_DAT_COLLECT);
    assert_eq!(file(g, "nd").base.dat_status, DatStatus::IN_DAT_NO_DUMP);
}

#[test]
fn step8_deduped_roms_are_merged() {
    let mut d = rom("d", 1, 1);
    d.status = "Deduped".into();
    let mut r = root(vec![game("g", vec![d, rom("n", 1, 2)])]);
    check_de_duped(&mut r);
    assert_eq!(
        file(dir(&r, "g"), "d").base.dat_status,
        DatStatus::IN_DAT_MERGED
    );
    assert_eq!(
        file(dir(&r, "g"), "n").base.dat_status,
        DatStatus::IN_DAT_COLLECT
    );
}

// ---- step 9 ----------------------------------------------------------------------------------

#[test]
fn step9_compression_method_table() {
    let hdr = |c: &str| DatHeader {
        compression: c.to_owned(),
        ..DatHeader::default()
    };
    let mut r = rule();
    let m = |r: &CleanRule, c: &str| compression_method(r, &hdr(c));
    // The rule's own choice when the DAT says nothing.
    assert_eq!(m(&r, ""), (FileType::ZIP, ZipStructure::ZIP_TRRNT));
    assert_eq!(m(&r, "TDC"), (FileType::ZIP, ZipStructure::ZIP_TDC));
    assert_eq!(m(&r, "unzip"), (FileType::DIR, ZipStructure::NONE));
    assert_eq!(m(&r, "File"), (FileType::DIR, ZipStructure::NONE));
    assert_eq!(m(&r, "fileonly"), (FileType::DIR, ZipStructure::NONE));
    assert_eq!(
        m(&r, "7z"),
        (FileType::SEVEN_ZIP, ZipStructure::SEVEN_ZIP_SLZMA)
    );
    assert_eq!(
        m(&r, "7ZIP"),
        (FileType::SEVEN_ZIP, ZipStructure::SEVEN_ZIP_SLZMA)
    );
    assert_eq!(m(&r, "zip"), (FileType::ZIP, ZipStructure::ZIP_TRRNT));
    assert_eq!(m(&r, "bogus"), (FileType::ZIP, ZipStructure::ZIP_TRRNT));
    r.compression_override_dat = true;
    assert_eq!(m(&r, "7z"), (FileType::ZIP, ZipStructure::ZIP_TRRNT));
    r.compression_override_dat = false;
    r.compression = FileType::SEVEN_ZIP;
    r.compression_sub = ZipStructure::SEVEN_ZIP_NZSTD;
    assert_eq!(
        m(&r, ""),
        (FileType::SEVEN_ZIP, ZipStructure::SEVEN_ZIP_NZSTD)
    );
    // FileOnly in the rule ignores the DAT; the pipeline then forces Dir/None.
    r.compression = FileType::FILE_ONLY;
    assert_eq!(
        m(&r, "zip"),
        (FileType::FILE, ZipStructure::SEVEN_ZIP_NZSTD)
    );
}

// ---- step 10 ---------------------------------------------------------------------------------

fn header_with(games: Vec<DatNode>) -> DatHeader {
    DatHeader {
        name: "Pack".into(),
        description: "Pack Desc".into(),
        base_dir: root(games),
        ..DatHeader::default()
    }
}

#[test]
fn step10_flatten_lifts_games_and_names_them_by_their_directories() {
    let mut inner = DatDir::new("Sub", FileType::UNSET);
    inner.child_add(game("g", vec![rom("r", 1, 1)]));
    let mut outer = DatDir::new("Top", FileType::UNSET);
    outer.child_add(DatNode::Dir(inner));
    let mut r = root(vec![
        DatNode::Dir(outer),
        DatNode::File(rom("loose", 1, 1)),
        game("h", vec![]),
    ]);
    directory_flatten(&mut r);
    assert_eq!(names(&r), ["Top/Sub/g", "h"], "loose files are dropped");
}

fn single_level(sub: RemoveSubType, is_files: bool, games: Vec<DatNode>) -> DatHeader {
    let mut h = header_with(games);
    make_dat_single_level(&mut h, false, sub, is_files, false, &[]);
    h
}

fn two_games() -> Vec<DatNode> {
    vec![
        game("A", vec![rom("one.rom", 1, 1)]),
        game("B", vec![rom("one.rom", 2, 2), rom("two.rom", 3, 3)]),
    ]
}

#[test]
fn step10_single_level_keep_all_adds_the_set_name_to_each_rom() {
    let h = single_level(RemoveSubType::KeepAllSubDirs, false, two_games());
    assert_eq!(h.dir, "noautodir");
    assert_eq!(names(&h.base_dir), ["Pack"]);
    let pack = dir(&h.base_dir, "Pack");
    assert_eq!(pack.game.as_ref().unwrap().description, "Pack Desc");
    assert_eq!(names(pack), ["A/one.rom", "B/one.rom", "B/two.rom"]);
}

#[test]
fn step10_single_level_remove_all_flattens_and_conflicts_collide() {
    let h = single_level(RemoveSubType::RemoveAllSubDirs, false, two_games());
    assert_eq!(
        names(dir(&h.base_dir, "Pack")),
        ["one.rom", "one.rom", "two.rom"]
    );
    // With a conflict check the same input falls back to keeping the sets.
    let h = single_level(RemoveSubType::RemoveAllIfNoConflicts, false, two_games());
    assert_eq!(
        names(dir(&h.base_dir, "Pack")),
        ["A/one.rom", "B/one.rom", "B/two.rom"]
    );
    let ok = vec![
        game("A", vec![rom("one.rom", 1, 1)]),
        game("B", vec![rom("two.rom", 3, 3)]),
    ];
    let h = single_level(RemoveSubType::RemoveAllIfNoConflicts, false, ok);
    assert_eq!(names(dir(&h.base_dir, "Pack")), ["one.rom", "two.rom"]);
}

#[test]
fn step10_single_level_single_file_and_name_match_rules() {
    let h = single_level(RemoveSubType::RemoveSubIfSingleFiles, false, two_games());
    assert_eq!(
        names(dir(&h.base_dir, "Pack")),
        ["one.rom", "B/one.rom", "B/two.rom"]
    );
    let games = vec![
        game("same", vec![rom("same.rom", 1, 1)]),
        game("other", vec![rom("different.rom", 2, 2)]),
    ];
    let h = single_level(RemoveSubType::RemoveSubIfNameMatches, false, games);
    assert_eq!(
        names(dir(&h.base_dir, "Pack")),
        ["same.rom", "other/different.rom"]
    );
}

#[test]
fn step10_single_level_with_files_keeps_directories_not_name_prefixes() {
    let h = single_level(RemoveSubType::KeepAllSubDirs, true, two_games());
    // No wrapper game: the sets stay directories directly under the root.
    assert_eq!(names(&h.base_dir), ["A", "B"]);
    assert_eq!(names(dir(&h.base_dir, "B")), ["one.rom", "two.rom"]);
    assert!(dir(&h.base_dir, "B").game.is_some());
    let h = single_level(RemoveSubType::RemoveAllSubDirs, true, two_games());
    assert_eq!(names(&h.base_dir), ["one.rom", "one.rom", "two.rom"]);
}

#[test]
fn step10_single_level_puts_the_category_into_the_rom_name() {
    let cat = DatGame {
        category: vec!["Puzzle".to_owned()],
        ..DatGame::default()
    };
    let mut h = header_with(vec![game_with("Pz", cat, vec![rom("p.rom", 1, 1)])]);
    make_dat_single_level(
        &mut h,
        true,
        RemoveSubType::RemoveAllSubDirs,
        false,
        true,
        &[],
    );
    // `use_description` names the wrapper after the description.
    assert_eq!(names(&h.base_dir), ["Pack Desc"]);
    assert_eq!(names(dir(&h.base_dir, "Pack Desc")), ["Puzzle/p.rom"]);
}

// ---- step 11 onwards -------------------------------------------------------------------------

fn typed(r: &mut DatDir, ft: FileType, zs: ZipStructure) {
    set_type(r, ft, zs, true).unwrap();
}

#[test]
fn step11_types_flags_and_sort_order() {
    let mut r = root(vec![
        game(
            "b game",
            vec![rom("b", 1, 1), rom("A", 2, 2), rom("a", 3, 3)],
        ),
        game("A game", vec![]),
    ]);
    typed(&mut r, FileType::ZIP, ZipStructure::ZIP_TRRNT);
    assert_eq!(r.base.file_type, FileType::DIR);
    // Games sort as directories, case-insensitively.
    assert_eq!(names(&r), ["A game", "b game"]);
    let g = dir(&r, "b game");
    assert_eq!(g.base.file_type, FileType::ZIP);
    assert_eq!(
        g.dat_struct,
        0x80 | ZipStructure::ZIP_TRRNT.0,
        "fix flag in bit 7"
    );
    // Inside a zip: TorrentZip order, case-folded first, then ordinal.
    assert_eq!(names(g), ["A", "a", "b"]);
    assert!(g
        .children
        .iter()
        .all(|c| c.base().file_type == FileType::FILE_ZIP));
}

#[test]
fn step11_dir_rule_makes_files_and_a_game_keeps_the_type_its_dat_gave_it() {
    let mut r = root(vec![game("g", vec![rom("x", 1, 1)])]);
    typed(&mut r, FileType::DIR, ZipStructure::NONE);
    assert_eq!(dir(&r, "g").base.file_type, FileType::DIR);
    assert_eq!(file(dir(&r, "g"), "x").base.file_type, FileType::FILE);
    assert_eq!(
        dir(&r, "g").dat_struct,
        0,
        "no archive structure for a directory"
    );

    let mut pre = DatDir::new("sevenz", FileType::SEVEN_ZIP);
    pre.game = Some(Box::default());
    pre.child_add(DatNode::File(rom("z", 1, 1)));
    let mut r = root(vec![DatNode::Dir(pre)]);
    typed(&mut r, FileType::ZIP, ZipStructure::ZIP_TRRNT);
    let g = dir(&r, "sevenz");
    assert_eq!(g.base.file_type, FileType::SEVEN_ZIP);
    assert_eq!(g.dat_struct & 0x7f, ZipStructure::SEVEN_ZIP_NZSTD.0);
    assert_eq!(file(g, "z").base.file_type, FileType::FILE_SEVEN_ZIP);
}

#[test]
fn step11_a_tdc_zip_whose_files_all_have_the_trrntzip_date_is_a_trrntzip() {
    let dated = |d: Option<i64>| {
        let mut f = rom("f", 1, 1);
        f.base.date_modified = d;
        f
    };
    let mut r = root(vec![game("all", vec![dated(Some(563_657_728))])]);
    typed(&mut r, FileType::ZIP, ZipStructure::ZIP_TDC);
    assert_eq!(dir(&r, "all").dat_struct & 0x7f, ZipStructure::ZIP_TRRNT.0);
    let mut r = root(vec![game(
        "some",
        vec![dated(Some(563_657_728)), dated(None)],
    )]);
    typed(&mut r, FileType::ZIP, ZipStructure::ZIP_TDC);
    assert_eq!(dir(&r, "some").dat_struct & 0x7f, ZipStructure::ZIP_TDC.0);
}

#[test]
fn step11_a_file_type_the_table_cannot_name_is_an_error() {
    let mut r = root(vec![game("g", vec![rom("x", 1, 1)])]);
    assert!(set_type(&mut r, FileType::FILE, ZipStructure::NONE, false).is_err());
}

#[test]
fn step12_empty_directory_entries_go_unless_the_zip_keeps_dates() {
    let make = |zs| {
        let mut r = root(vec![game(
            "g",
            vec![
                rom("d/", 0, 0),
                rom("d/f", 1, 1),
                rom("e/", 0, 0),
                rom("lonely/", 0, 0),
            ],
        )]);
        typed(&mut r, FileType::ZIP, zs);
        remove_un_needed_directories(&mut r).unwrap();
        r
    };
    let r = make(ZipStructure::ZIP_TRRNT);
    // `d/` has a file below it and goes; `e/` and `lonely/` do not and stay.
    assert_eq!(names(dir(&r, "g")), ["d/f", "e/", "lonely/"]);
    let r = make(ZipStructure::ZIP_TDC);
    assert_eq!(names(dir(&r, "g")), ["d/", "d/f", "e/", "lonely/"]);
}

#[test]
fn step13_dates_survive_only_in_tdc_dtd_dtz_zips() {
    let dated = |name: &str| {
        let mut f = rom(name, 1, 1);
        f.base.date_modified = Some(5);
        f
    };
    let mut r = root(vec![
        game("plain", vec![dated("a")]),
        game("tdc", vec![dated("b")]),
    ]);
    typed(&mut r, FileType::ZIP, ZipStructure::ZIP_TRRNT);
    if let DatNode::Dir(d) = &mut r.children[1] {
        d.dat_struct = ZipStructure::ZIP_TDC.0;
    }
    remove_all_date_time(&mut r);
    assert_eq!(file(dir(&r, "plain"), "a").base.date_modified, None);
    assert_eq!(file(dir(&r, "tdc"), "b").base.date_modified, Some(5));
}

#[test]
fn step14_a_disk_in_an_archive_moves_next_to_it_as_a_loose_file() {
    let mut disk = DatFile::new("img");
    disk.is_disk = true;
    let mut r = root(vec![game("G", vec![rom("r.bin", 1, 1), disk])]);
    typed(&mut r, FileType::ZIP, ZipStructure::ZIP_TRRNT);
    move_up_chds(&mut r);
    assert_eq!(
        names(&r),
        ["G", "G"],
        "the archive and a directory of the same name"
    );
    let archive = r.children.iter().find_map(|c| match c {
        DatNode::Dir(d) if d.base.file_type == FileType::ZIP => Some(d),
        _ => None,
    });
    assert_eq!(names(archive.unwrap()), ["r.bin"]);
    let folder = r.children.iter().find_map(|c| match c {
        DatNode::Dir(d) if d.base.file_type == FileType::DIR => Some(d),
        _ => None,
    });
    let folder = folder.unwrap();
    assert!(
        folder.game.is_some(),
        "the folder keeps the game information"
    );
    assert_eq!(file(folder, "img").base.file_type, FileType::FILE);
}

#[test]
fn step15_slashes_in_names_become_directories_except_inside_archives() {
    let mut r = root(vec![
        game("Series/One", vec![rom("x", 1, 1)]),
        game("Series/Two", vec![rom("y/z", 2, 2)]),
        game("Solo", vec![]),
    ]);
    typed(&mut r, FileType::DIR, ZipStructure::NONE);
    directory_expand(&mut r);
    assert_eq!(names(&r), ["Series", "Solo"]);
    let series = dir(&r, "Series");
    assert_eq!(names(series), ["One", "Two"]);
    assert_eq!(
        names(dir(series, "Two")),
        ["y"],
        "files expand in a directory game"
    );
    assert_eq!(names(dir(dir(series, "Two"), "y")), ["z"]);

    let mut r = root(vec![game("Series/One", vec![rom("y/z", 2, 2)])]);
    typed(&mut r, FileType::ZIP, ZipStructure::ZIP_TRRNT);
    directory_expand(&mut r);
    assert_eq!(
        names(dir(dir(&r, "Series"), "One")),
        ["y/z"],
        "an archive keeps its entry names"
    );
}

#[test]
fn steps16_and_17_trimmed_names_blank_names_and_duplicates() {
    let mut r = root(vec![
        game("  Spaced ", vec![rom(" lead. ", 1, 1)]),
        game("dots..", vec![]),
        game("  ", vec![]),
        game(
            "Same",
            vec![rom("a.rom", 1, 1), rom("A.ROM", 2, 2), rom("a.rom", 3, 3)],
        ),
        game("same", vec![]),
        game("SAME", vec![]),
    ]);
    typed(&mut r, FileType::DIR, ZipStructure::NONE);
    clean_file_names_full(&mut r);
    // A Dir keeps no trailing spaces or dots; an empty name becomes `_`.
    let n = names(&r);
    assert!(n.contains(&"Spaced"), "{n:?}");
    assert!(n.contains(&"dots"), "{n:?}");
    assert!(n.contains(&"_"), "{n:?}");
    assert_eq!(names(dir(&r, "Spaced")), ["lead"]);

    fix_dupes(&mut r);
    // The three games called Same differ only by case. Directories sort upper case first
    // (SAME, Same, same), so the first keeps its name and the later two are renamed.
    let same: Vec<&str> = names(&r)
        .into_iter()
        .filter(|n| n.to_lowercase().starts_with("same"))
        .collect();
    assert_eq!(same, ["SAME", "Same_0", "same_1"]);
    // The files of the game that was called Same: `base_<n>.ext`, in the order A.ROM, a.rom,
    // a.rom.
    assert_eq!(names(dir(&r, "Same_0")), ["A.ROM", "a_0.rom", "a_1.rom"]);
}

#[test]
fn step18_games_without_roms_are_dropped() {
    let mut sub = DatDir::new("dir", FileType::UNSET);
    sub.child_add(game("empty", vec![]));
    let mut r = root(vec![
        game("full", vec![rom("r", 1, 1)]),
        game("empty", vec![]),
        DatNode::Dir(sub),
    ]);
    remove_empty_directories(&mut r);
    assert_eq!(names(&r), ["full", "dir"]);
    assert!(dir(&r, "dir").children.is_empty());
}

#[test]
fn set_ext_adds_archive_and_chd_extensions_and_resorts() {
    let mut disk = DatFile::new("disk");
    disk.is_disk = true;
    disk.merge = "parentdisk".into();
    let mut r = root(vec![
        game("b", vec![rom("x", 1, 1), disk]),
        game("a", vec![]),
    ]);
    typed(&mut r, FileType::SEVEN_ZIP, ZipStructure::SEVEN_ZIP_SLZMA);
    set_ext(&mut r, HeaderFileType::NES);
    assert_eq!(names(&r), ["a.7z", "b.7z"]);
    let g = dir(&r, "b.7z");
    let d = file(g, "disk.chd");
    assert_eq!(d.header_file_type, HeaderFileType::CHD);
    assert_eq!(d.merge, "parentdisk.chd");
    assert_eq!(file(g, "x").header_file_type, HeaderFileType::NES);
}

#[test]
fn clear_description_blanks_a_description_that_repeats_the_name() {
    let g = |name: &str, desc: &str| {
        game_with(
            name,
            DatGame {
                description: desc.to_owned(),
                ..DatGame::default()
            },
            vec![],
        )
    };
    let mut r = root(vec![g("Game.zip", "Game"), g("Other.zip", "Other (long)")]);
    clear_description(&mut r);
    assert_eq!(
        dir(&r, "Game.zip").game.as_ref().unwrap().description,
        "\u{a4}"
    );
    assert_eq!(
        dir(&r, "Other.zip").game.as_ref().unwrap().description,
        "Other (long)"
    );
}

// ---- step 6 ----------------------------------------------------------------------------------

fn family() -> DatDir {
    let bios_info = DatGame {
        is_bios: "yes".into(),
        ..DatGame::default()
    };
    root(vec![
        game_with("bios", bios_info, vec![rom("b.rom", 10, 0xb105)]),
        game_with(
            "parent",
            DatGame {
                rom_of: "bios".into(),
                ..DatGame::default()
            },
            vec![rom("b.rom", 10, 0xb105), rom("p.rom", 20, 0x0102)],
        ),
        clone_game(
            "clone",
            "parent",
            vec![
                rom("b.rom", 10, 0xb105),
                rom("p.rom", 20, 0x0102),
                rom("c.rom", 30, 0x0c0c),
            ],
        ),
    ])
}

#[test]
fn has_rom_of_looks_at_romof_cloneof_and_cloneofid() {
    assert!(has_rom_of(&family()));
    assert!(!has_rom_of(&root(vec![game("g", vec![])])));
    let by_id = DatGame {
        clone_of_id: "7".into(),
        ..DatGame::default()
    };
    assert!(has_rom_of(&root(vec![game_with("g", by_id, vec![])])));
}

#[test]
fn match_ids_turns_cloneofid_into_cloneof() {
    let mut r = root(vec![
        game_with(
            "Parent",
            DatGame {
                id: "100".into(),
                ..DatGame::default()
            },
            vec![],
        ),
        game_with(
            "Child",
            DatGame {
                id: "101".into(),
                clone_of_id: "100".into(),
                ..DatGame::default()
            },
            vec![],
        ),
        game_with(
            "Orphan",
            DatGame {
                clone_of_id: "999".into(),
                ..DatGame::default()
            },
            vec![],
        ),
    ]);
    set_match_ids(&mut r).unwrap();
    assert_eq!(dir(&r, "Child").game.as_ref().unwrap().clone_of, "Parent");
    assert_eq!(dir(&r, "Orphan").game.as_ref().unwrap().clone_of, "");
}

#[test]
fn merge_set_moves_clone_roms_into_the_top_parent_and_drops_bios_roms() {
    let mut r = family();
    make_merge_set(&mut r, false).unwrap();
    // The clone lost b.rom (the BIOS has it); its other ROMs move up into the top non-BIOS
    // parent, so the clone is empty.
    assert!(dir(&r, "clone").children.is_empty());
    // The parent is itself a clone of the BIOS, so it loses b.rom too, and receives the clone's
    // p.rom and c.rom. (The two p.rom are the same ROM; `remove_dupes` takes one out later.)
    let mut got: Vec<&str> = names(dir(&r, "parent"));
    got.sort_unstable();
    assert_eq!(got, ["c.rom", "p.rom", "p.rom"]);
    // The BIOS is never touched.
    assert_eq!(names(dir(&r, "bios")), ["b.rom"]);
}

#[test]
fn merge_set_names_roms_after_the_game_for_mame_xml() {
    let mut r = family();
    make_merge_set(&mut r, true).unwrap();
    let parent = dir(&r, "parent");
    assert!(
        names(parent).contains(&"clone/c.rom"),
        "{:?}",
        names(parent)
    );
}

#[test]
fn split_set_marks_parent_roms_in_the_clone_as_merged() {
    let mut r = family();
    make_split_set(&mut r).unwrap();
    let c = dir(&r, "clone");
    assert_eq!(names(c).len(), 3, "a split set keeps every ROM in place");
    for n in ["b.rom", "p.rom"] {
        assert_eq!(file(c, n).base.dat_status, DatStatus::IN_DAT_MERGED, "{n}");
        assert_eq!(file(c, n).merge, "(Auto Merged)");
    }
    assert_eq!(file(c, "c.rom").base.dat_status, DatStatus::IN_DAT_COLLECT);
    assert_eq!(
        file(dir(&r, "parent"), "b.rom").base.dat_status,
        DatStatus::IN_DAT_MERGED
    );
}

#[test]
fn non_merge_set_copies_device_roms_once() {
    let dev = DatGame {
        is_device: "yes".into(),
        ..DatGame::default()
    };
    let mut sha = rom("dev.rom", 4, 9);
    sha.sha1 = Some(vec![7; 20]);
    let uses = DatGame {
        device_ref: vec!["cpu".into()],
        ..DatGame::default()
    };
    let mut r = root(vec![
        game_with("cpu", dev, vec![sha.clone()]),
        game_with("machine", uses, vec![sha]),
        game_with(
            "machine2",
            DatGame {
                device_ref: vec!["cpu".into()],
                ..DatGame::default()
            },
            vec![rom("own.rom", 1, 1)],
        ),
    ]);
    make_non_merge_set(&mut r).unwrap();
    // machine already has dev.rom (same SHA1 and name): no copy; machine2 gets one.
    assert_eq!(names(dir(&r, "machine")), ["dev.rom"]);
    assert_eq!(names(dir(&r, "machine2")), ["own.rom", "dev.rom"]);
    remove_device_ref(&mut r);
    assert!(dir(&r, "machine")
        .game
        .as_ref()
        .unwrap()
        .device_ref
        .is_empty());
}

#[test]
fn remove_dupes_prefers_plain_names_and_keeps_different_content() {
    let mut r = root(vec![game(
        "g",
        vec![
            rom("a.rom", 1, 0xAA),
            rom("a.rom", 1, 0xAA),
            rom("dir/a.rom", 1, 0xAA),
            rom("b.rom", 2, 0xBB),
            rom("b.rom", 2, 0xCC),
        ],
    )]);
    remove_dupes(&mut r, true, false).unwrap();
    // The exact duplicate goes. With names compared, `dir/a.rom` is a different ROM; b.rom with
    // other content is not a duplicate.
    let mut got = names(dir(&r, "g"));
    got.sort_unstable();
    assert_eq!(got, ["a.rom", "b.rom", "b.rom", "dir/a.rom"]);

    // Without `test_name` (MAME XML) the same content under another name is a duplicate when one
    // of the names has a `/`: that one loses.
    let mut r = root(vec![game(
        "g",
        vec![rom("x/a.rom", 1, 0xAA), rom("a.rom", 1, 0xAA)],
    )]);
    remove_dupes(&mut r, false, false).unwrap();
    assert_eq!(names(dir(&r, "g")), ["a.rom"]);
    // Two plain names are left alone, unless the second names the first as its merge name.
    let mut renamed = rom("new.rom", 1, 0xAA);
    renamed.merge = "old.rom".into();
    let mk = || {
        root(vec![game(
            "g",
            vec![rom("old.rom", 1, 0xAA), renamed.clone()],
        )])
    };
    let mut r = mk();
    remove_dupes(&mut r, false, false).unwrap();
    assert_eq!(names(dir(&r, "g")), ["old.rom", "new.rom"]);
    let mut r = mk();
    remove_dupes(&mut r, false, true).unwrap();
    assert_eq!(names(dir(&r, "g")), ["old.rom"]);
}

#[test]
fn empty_sets_are_removed_recursively_but_the_root_stays() {
    let mut inner = DatDir::new("dir", FileType::UNSET);
    inner.child_add(game("hollow", vec![]));
    let mut r = root(vec![
        game("full", vec![rom("r", 1, 1)]),
        DatNode::Dir(inner),
        game("hollow2", vec![]),
    ]);
    assert!(remove_empty_sets(&mut r));
    assert_eq!(names(&r), ["full"]);
    let mut none = root(vec![game("hollow", vec![])]);
    assert!(!remove_empty_sets(&mut none));
    assert!(none.children.is_empty());
}

// ---- the whole pipeline ------------------------------------------------------------------------

fn pipeline(
    text: &str,
    mut rule: CleanRule,
    tweak: impl FnOnce(&mut CleanRule),
) -> (DatHeader, HeaderFileType) {
    tweak(&mut rule);
    let mut h = crate::read_dat(text.as_bytes(), "t.dat").expect("parses");
    run(&rule, &mut h).expect("cleans");
    let hft = finish(&rule, &mut h, None);
    (h, hft)
}

const CMP: &str = r#"clrmamepro (
	name "Test Set"
	description "A test"
	forcepacking zip
	header "No-Intro_NES.xml"
)
game (
	name "Parent (World)"
	description "Parent (World)"
	rom ( name "a.nes" size 16 crc 0000000a )
	rom ( name "b.nes" size 16 crc 0000000b status nodump )
	disk ( name "img" sha1 0123456789012345678901234567890123456789 )
)
game (
	name "Child (Japan)"
	description "The Child"
	cloneof "Parent (World)"
	romof "Parent (World)"
	rom ( name "a.nes" size 16 crc 0000000a )
	rom ( name "c.nes" size 16 crc 0000000c )
)
"#;

#[test]
fn pipeline_default_rule_builds_trrntzip_games_with_extensions_and_header_types() {
    let (h, hft) = pipeline(CMP, rule(), |_| {});
    assert_eq!(hft, HeaderFileType::NES);
    let root = &h.base_dir;
    // The disk of the parent moved out of its zip into a directory of the same name (sorted
    // before the zip, because the directory name is a prefix of the archive's).
    assert_eq!(
        names(root),
        ["Child (Japan).zip", "Parent (World)", "Parent (World).zip"]
    );

    let parent = dir(root, "Parent (World).zip");
    assert_eq!(parent.base.file_type, FileType::ZIP);
    assert_eq!(parent.dat_struct, 0x80 | ZipStructure::ZIP_TRRNT.0);
    // b.nes is a no-dump ROM and is gone; the others carry the DAT's copier-header type.
    assert_eq!(names(parent), ["a.nes"]);
    let a = file(parent, "a.nes");
    assert_eq!(a.header_file_type, HeaderFileType::NES);
    assert_eq!(a.base.dat_status, DatStatus::IN_DAT_COLLECT);
    assert_eq!(a.base.file_type, FileType::FILE_ZIP);

    let folder = dir(root, "Parent (World)");
    assert_eq!(folder.base.file_type, FileType::DIR);
    let img = file(folder, "img.chd");
    assert_eq!(img.header_file_type, HeaderFileType::CHD);
    assert_eq!(img.base.file_type, FileType::FILE);

    // No merge rule: the clone keeps its own copy of a.nes, and its description differs from its
    // name so it stays.
    let child = dir(root, "Child (Japan).zip");
    assert_eq!(names(child), ["a.nes", "c.nes"]);
    assert_eq!(child.game.as_ref().unwrap().description, "The Child");
    assert_eq!(
        dir(root, "Parent (World).zip")
            .game
            .as_ref()
            .unwrap()
            .description,
        "\u{a4}",
        "a description that only repeats the name is blanked"
    );
}

#[test]
fn pipeline_headerless_rule_drops_the_header_type_and_headered_requires_it() {
    let (_, hft) = pipeline(CMP, rule(), |r| r.header_type = HeaderType::Headerless);
    assert_eq!(hft, HeaderFileType::NOTHING);
    let (h, hft) = pipeline(CMP, rule(), |r| r.header_type = HeaderType::Headered);
    assert_eq!(hft.0, HeaderFileType::NES.0 | HeaderFileType::REQUIRED.0);
    let parent = dir(&h.base_dir, "Parent (World).zip");
    assert_eq!(file(parent, "a.nes").header_file_type.0, 0x88);
}

#[test]
fn pipeline_merge_rule_moves_the_clone_into_the_parent() {
    let (h, _) = pipeline(CMP, rule(), |r| r.merge = MergeType::Merge);
    let root = &h.base_dir;
    assert!(
        !names(root).contains(&"Child (Japan).zip"),
        "the clone's ROMs are in the parent's zip: {:?}",
        names(root)
    );
    assert_eq!(names(dir(root, "Parent (World).zip")), ["a.nes", "c.nes"]);
}

#[test]
fn pipeline_split_rule_marks_merged_roms() {
    let (h, _) = pipeline(CMP, rule(), |r| r.merge = MergeType::Split);
    let child = dir(&h.base_dir, "Child (Japan).zip");
    assert_eq!(
        file(child, "a.nes").base.dat_status,
        DatStatus::IN_DAT_MERGED
    );
    assert_eq!(file(child, "a.nes").merge, "(Auto Merged)");
    assert_eq!(
        file(child, "c.nes").base.dat_status,
        DatStatus::IN_DAT_COLLECT
    );
}

#[test]
fn pipeline_chds_only_filter_keeps_only_the_disk() {
    let (h, _) = pipeline(CMP, rule(), |r| r.filter = FilterType::ChdsOnly);
    let names: Vec<&str> = names(&h.base_dir);
    assert_eq!(names, ["Parent (World)"], "the clone has no disks and goes");
}

#[test]
fn pipeline_file_only_rule_makes_directories_of_loose_files() {
    let (h, _) = pipeline(CMP, rule(), |r| r.compression = FileType::FILE_ONLY);
    let parent = dir(&h.base_dir, "Parent (World)");
    assert_eq!(parent.base.file_type, FileType::DIR);
    assert_eq!(file(parent, "a.nes").base.file_type, FileType::FILE);
    assert_eq!(parent.dat_struct, 0);
}

#[test]
fn finish_wraps_an_auto_directory_around_the_dat() {
    let rule = rule();
    let mut h = crate::read_dat(CMP.as_bytes(), "t.dat").unwrap();
    run(&rule, &mut h).unwrap();
    finish(&rule, &mut h, Some("My: Dat"));
    assert_eq!(names(&h.base_dir), ["My- Dat"]);
    assert_eq!(h.base_dir.base.name, "");
    assert_eq!(dir(&h.base_dir, "My- Dat").base.file_type, FileType::DIR);
}
