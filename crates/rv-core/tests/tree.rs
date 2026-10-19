//! The database tree primitives: removing what a scan knew, taking a DAT out, counting statuses,
//! and linking nodes to their DATs.

use rv_core::{
    ui_status, Cx, DatStatus, Database, DbError, DirStatus, EFile, FileStatus, FileType, GotStatus,
    HeaderFileType, MetaEntry, MiaStatus, RepStatus, RvDat, RvFile, Settings, ZipStructure,
};

fn cx(s: &Settings) -> Cx<'_> {
    Cx::new(s, "RomVault/dir")
}

/// A DAT file as `RvFile(DatBase)` builds it: size and CRC from the DAT, not yet seen on disk.
fn dat_file(name: &str) -> RvFile {
    let mut f = RvFile::new(FileType::FILE_ZIP, name);
    f.size = Some(10);
    f.crc = Some(vec![1, 2, 3, 4]);
    f.dat_status = DatStatus::IN_DAT_COLLECT;
    f.file_status = FileStatus::SIZE_FROM_DAT.with(FileStatus::CRC_FROM_DAT);
    f
}

/// What a scan adds to a DAT file it found.
fn found(mut f: RvFile) -> RvFile {
    f.got_status = GotStatus::GOT;
    f.sha1 = Some(vec![9; 20]);
    f.md5 = Some(vec![8; 16]);
    f.zip_file_index = Some(3);
    f.zip_file_header_position = Some(77);
    f.file_mod_time_stamp = 1234;
    f.file_name = "Other".into();
    f.chd_version = Some(5);
    f.set_header_type_raw(HeaderFileType::NES);
    f.alt_size = Some(5);
    f.alt_crc = Some(vec![7; 4]);
    f.file_status = f
        .file_status
        .with(FileStatus::SIZE_VERIFIED)
        .with(FileStatus::CRC_VERIFIED)
        .with(FileStatus::SHA1_VERIFIED)
        .with(FileStatus::ALT_CRC_FROM_HEADER)
        .with(FileStatus::HEADER_FILE_TYPE_FROM_HEADER);
    f
}

#[test]
fn file_remove_keeps_what_the_dat_said_and_forgets_what_the_scan_found() {
    let s = Settings::default();
    let mut f = found(dat_file("a.rom"));
    f.zip_struct = Some(ZipStructure::ZIP_TRRNT);
    assert_eq!(f.file_remove(&cx(&s), GotStatus::GOT), EFile::Keep);
    assert_eq!(f.got_status, GotStatus::NOT_GOT);
    assert_eq!(f.rep_status, RepStatus::MISSING);
    assert_eq!((f.size, f.crc.clone()), (Some(10), Some(vec![1, 2, 3, 4])));
    assert_eq!((f.sha1.clone(), f.md5.clone()), (None, None));
    assert_eq!(
        (f.alt_size, f.alt_crc.clone(), f.chd_version),
        (None, None, None)
    );
    assert_eq!(f.zip_file_index, None);
    assert_eq!(f.zip_file_header_position, None);
    assert_eq!(f.file_mod_time_stamp, i64::MIN);
    assert_eq!(f.file_name, "");
    assert_eq!(f.header_type(), HeaderFileType::NOTHING);
    assert_eq!(
        f.file_status,
        FileStatus::SIZE_FROM_DAT.with(FileStatus::CRC_FROM_DAT)
    );
}

#[test]
fn file_remove_promotes_alt_values_that_came_from_the_dat() {
    // A headered file was matched through its alt hashes: the DAT's values sit in Alt*.
    let s = Settings::default();
    let mut f = dat_file("a.nes");
    f.size = Some(272);
    f.crc = Some(vec![0xee; 4]);
    f.alt_size = Some(256);
    f.alt_crc = Some(vec![1, 1, 1, 1]);
    f.file_status = FileStatus::ALT_SIZE_FROM_DAT.with(FileStatus::ALT_CRC_FROM_DAT);
    f.got_status = GotStatus::GOT;
    f.file_remove(&cx(&s), GotStatus::GOT);
    assert_eq!((f.size, f.crc.clone()), (Some(256), Some(vec![1, 1, 1, 1])));
    assert_eq!((f.alt_size, f.alt_crc.clone()), (None, None));
    assert!(f
        .file_status
        .has(FileStatus::SIZE_FROM_DAT.with(FileStatus::CRC_FROM_DAT)));
}

#[test]
fn an_item_no_dat_knows_is_deleted_and_a_locked_parent_keeps_its_children_locked() {
    let s = Settings::default();
    let mut unknown = found(dat_file("u"));
    unknown.dat_status = DatStatus::NOT_IN_DAT;
    assert_eq!(unknown.file_remove(&cx(&s), GotStatus::GOT), EFile::Delete);
    let mut in_sort = found(dat_file("t"));
    in_sort.dat_status = DatStatus::IN_TO_SORT;
    assert_eq!(in_sort.file_remove(&cx(&s), GotStatus::GOT), EFile::Delete);

    let mut f = found(dat_file("a"));
    assert_eq!(f.file_remove(&cx(&s), GotStatus::FILE_LOCKED), EFile::Keep);
    assert_eq!(f.got_status, GotStatus::FILE_LOCKED);
    assert_eq!(f.rep_status, RepStatus::UN_SCANNED);
}

#[test]
fn a_date_from_the_dat_survives_a_removal() {
    let s = Settings::default();
    let mut f = found(dat_file("a"));
    f.file_mod_time_stamp = 555;
    f.file_status_set(FileStatus::DATE_FROM_DAT);
    f.file_remove(&cx(&s), GotStatus::GOT);
    assert_eq!(f.file_mod_time_stamp, 555);
}

#[test]
fn mark_as_missing_deletes_unknown_children_and_recurses_into_archives_and_untreed_dirs() {
    let s = Settings::default();
    let mut zip = RvFile::new(FileType::ZIP, "g.zip");
    zip.zip_struct = Some(ZipStructure::ZIP_TRRNT);
    zip.got_status = GotStatus::GOT;
    zip.dat_status = DatStatus::IN_DAT_COLLECT;
    zip.children.push(found(dat_file("keep")));
    let mut stranger = found(dat_file("stranger"));
    stranger.dat_status = DatStatus::NOT_IN_DAT;
    zip.children.push(stranger);
    let mut plain = RvFile::new(FileType::DIR, "plain");
    plain.dat_status = DatStatus::IN_DAT_COLLECT;
    plain.got_status = GotStatus::GOT;
    let mut inner = RvFile::new(FileType::FILE, "inner");
    inner.dat_status = DatStatus::IN_DAT_COLLECT;
    inner.got_status = GotStatus::GOT;
    plain.children.push(inner);
    let mut row = RvFile::new(FileType::DIR, "row");
    row.tree = Some(rv_core::RvTreeRow::default());
    row.dat_status = DatStatus::IN_DAT_COLLECT;
    let mut row_child = RvFile::new(FileType::FILE, "rc");
    row_child.dat_status = DatStatus::IN_DAT_COLLECT;
    row_child.got_status = GotStatus::GOT;
    row.children.push(row_child);
    let mut top = RvFile::new(FileType::DIR, "top");
    top.children = vec![zip, plain, row];

    top.mark_as_missing(&Cx::new(&s, "RomVault"));
    let zip = &top.children[0];
    assert_eq!(zip.children.len(), 1, "the unknown entry is gone");
    assert_eq!(zip.children[0].got_status, GotStatus::NOT_GOT);
    assert_eq!(
        zip.got_status,
        GotStatus::NOT_GOT,
        "the archive is a child of `top`"
    );
    assert_eq!(
        top.got_status,
        GotStatus::NOT_GOT,
        "`top` itself is the caller's to mark"
    );
    assert_eq!(top.children[1].children[0].got_status, GotStatus::NOT_GOT);
    assert_eq!(
        top.children[2].children[0].got_status,
        GotStatus::GOT,
        "a tree row is scanned on its own"
    );
}

#[test]
fn dat_remove_turns_an_item_into_an_unknown_one_or_deletes_it() {
    let s = Settings::default();
    let mut f = found(dat_file("a"));
    f.dat_uid = 4;
    f.merge = Some("m".into());
    f.status = Some("good".into());
    f.mia_status = Some(MiaStatus::MIA_FROM_DAT.with(MiaStatus::NEW));
    f.rep_status = RepStatus::CORRECT;
    f.set_header_type_raw(HeaderFileType(
        HeaderFileType::NES.0 | HeaderFileType::REQUIRED.0,
    ));
    assert_eq!(f.dat_remove(&cx(&s)), EFile::Keep);
    assert_eq!(f.dat_status, DatStatus::NOT_IN_DAT);
    assert_eq!(f.rep_status, RepStatus::UNKNOWN);
    assert_eq!(
        (f.dat_uid, f.merge.clone(), f.status.clone()),
        (0, None, None)
    );
    assert_eq!(f.mia_status, Some(MiaStatus::NEW), "only MIAFromDat goes");
    assert_eq!(f.name, "Other", "the name on disk is the name again");
    assert_eq!(f.file_name, "");
    // Hashes the scan verified stay; the DAT's size and CRC would not have, but these were verified.
    assert_eq!(
        (f.size, f.crc.is_some(), f.sha1.is_some()),
        (Some(10), true, true)
    );
    assert!(!f.file_status.has(FileStatus::SIZE_FROM_DAT));
    assert!(!f.header_type_required());

    let mut missing = dat_file("m");
    missing.dat_uid = 4;
    assert_eq!(missing.dat_remove(&cx(&s)), EFile::Delete);
    assert_eq!(
        (missing.size, missing.crc),
        (None, None),
        "the DAT's values go with it"
    );
}

#[test]
fn rep_status_follows_dat_and_got_and_the_ignore_rule_only_touches_unknown_loose_files() {
    let mut s = Settings::default();
    s.dat_rules[0].ignore_files = vec!["*.nfo".into()];
    s.set_regex_rules().unwrap();
    let c = Cx::new(&s, "RomVault/Console");
    let mut f = RvFile::new(FileType::FILE, "x.NFO");
    f.set_dat_got_status(DatStatus::NOT_IN_DAT, GotStatus::GOT, &c);
    assert_eq!(f.rep_status, RepStatus::IGNORE);
    // Once it is a DAT file the pattern no longer applies.
    f.set_dat_got_status(DatStatus::IN_DAT_COLLECT, GotStatus::GOT, &c);
    assert_eq!(f.rep_status, RepStatus::CORRECT);
    // An archive member is not a loose file.
    let mut m = RvFile::new(FileType::FILE_ZIP, "x.nfo");
    m.set_dat_got_status(DatStatus::NOT_IN_DAT, GotStatus::GOT, &c);
    assert_eq!(m.rep_status, RepStatus::UNKNOWN);
    // A status that is neither UnSet, Unknown nor Ignore keeps the pattern from applying: the
    // quirk of `RvFile.RepStatusReset` (a fix-time status is not relabelled by a reset).
    let mut moving = RvFile::new(FileType::FILE, "y.nfo");
    moving.rep_status = RepStatus::MOVE_TO_SORT;
    moving.set_dat_got_status(DatStatus::NOT_IN_DAT, GotStatus::GOT, &c);
    assert_eq!(moving.rep_status, RepStatus::UNKNOWN);
    // An uncoded combination is an error status.
    let mut odd = RvFile::new(FileType::FILE, "z");
    odd.set_dat_got_status(DatStatus::IN_DAT_NO_DUMP, GotStatus::NOT_GOT, &c);
    assert_eq!(odd.rep_status, RepStatus::ERROR);
    // Where no rule covers the item (ToSort) the global patterns are used.
    s.ignore_files = vec!["*.tmp".into()];
    s.set_regex_rules().unwrap();
    let t = Cx::new(&s, "ToSort");
    let mut g = RvFile::new(FileType::FILE, "q.tmp");
    g.set_dat_got_status(DatStatus::IN_TO_SORT, GotStatus::GOT, &t);
    assert_eq!(
        g.rep_status,
        RepStatus::IN_TO_SORT,
        "ToSort items are not NotInDat"
    );
    let mut g2 = RvFile::new(FileType::FILE, "q.tmp");
    g2.set_dat_got_status(DatStatus::NOT_IN_DAT, GotStatus::GOT, &t);
    assert_eq!(g2.rep_status, RepStatus::IGNORE);
}

#[test]
fn counters_count_each_descendant_once_and_mia_changes_the_bucket() {
    let mut zip = RvFile::new(FileType::ZIP, "z");
    let mut ok = RvFile::new(FileType::FILE_ZIP, "ok");
    ok.rep_status = RepStatus::CORRECT;
    let mut mia_ok = RvFile::new(FileType::FILE_ZIP, "mia");
    mia_ok.rep_status = RepStatus::CORRECT;
    mia_ok.mia_status = Some(MiaStatus::MIA_FROM_DAT);
    let mut gone = RvFile::new(FileType::FILE_ZIP, "gone");
    gone.rep_status = RepStatus::MISSING;
    zip.children = vec![ok, mia_ok, gone];
    zip.rep_status = RepStatus::DIR_CORRECT;
    let mut dir = RvFile::new(FileType::DIR, "d");
    dir.children = vec![zip];

    let c: DirStatus = dir.dir_status(true);
    assert_eq!(
        c.get(RepStatus::DIR_CORRECT),
        1,
        "the archive counts for its parent"
    );
    assert_eq!(c.get(RepStatus::CORRECT), 1);
    assert_eq!(c.get(RepStatus::CORRECT_MIA), 1);
    assert_eq!(c.get(RepStatus::MISSING), 1);
    assert_eq!(c.count_correct(), 2);
    assert_eq!(c.count_missing(false), 1);
    assert!(c.has_correct() && c.has_missing(false) && !c.has_mia());
    assert_eq!(c.non_zero().len(), 4);
    // The directory's own status is not in its own counters; the archive's counters hold
    // only what is below the archive.
    assert_eq!(
        dir.children[0].dir_status(true).get(RepStatus::DIR_CORRECT),
        0
    );

    assert_eq!(
        ui_status(MiaStatus::MIA_FROM_DAT, RepStatus::MISSING, false),
        RepStatus::MISSING_MIA
    );
    assert_eq!(
        ui_status(MiaStatus::MIA, RepStatus::CAN_BE_FIXED, false),
        RepStatus::CAN_BE_FIXED_MIA
    );
    assert_eq!(
        ui_status(MiaStatus::NEW, RepStatus::MISSING, true),
        RepStatus::MISSING_NEW
    );
    assert_eq!(
        ui_status(MiaStatus::NEW, RepStatus::MISSING, false),
        RepStatus::MISSING
    );
    assert_eq!(
        ui_status(MiaStatus::MIA_FROM_DAT, RepStatus::UNKNOWN, true),
        RepStatus::UNKNOWN
    );

    let mut d = DirStatus::default();
    d.add(RepStatus::MOVE_TO_SORT, MiaStatus::NONE, true);
    d.add(RepStatus::NEEDED_FOR_FIX, MiaStatus::NONE, true);
    assert!(
        d.has_fixes_needed()
            && d.fix_check_has_needed_for_fix()
            && d.fix_check_has_files_to_be_removed()
    );
    assert_eq!(d.count_fixes_needed(), 2);
    assert!(d.has_fixable() && !d.fix_check_files_can_be_fixed());
    assert_eq!(d.count_any_files(), 2);
    let mut merged = DirStatus::default();
    merged.add(RepStatus::NOT_COLLECTED, MiaStatus::NONE, true);
    assert!(merged.has_all_merged());
    merged.add_all(&d);
    assert!(!merged.has_all_merged());
}

#[test]
fn is_deep_scanned_needs_every_hash_verified() {
    let s = Settings::default();
    let mut f = RvFile::new(FileType::FILE, "f");
    assert!(!f.is_deep_scanned(&s));
    f.file_status = FileStatus::SIZE_VERIFIED
        .with(FileStatus::CRC_VERIFIED)
        .with(FileStatus::SHA1_VERIFIED)
        .with(FileStatus::MD5_VERIFIED);
    assert!(f.is_deep_scanned(&s));
    // An alt hash taken from a header but never verified (a CHD that was not decoded).
    f.file_status = f.file_status.with(FileStatus::ALT_SHA1_FROM_HEADER);
    assert!(!f.is_deep_scanned(&s));
    f.file_status = f.file_status.with(FileStatus::ALT_SHA1_VERIFIED);
    assert!(f.is_deep_scanned(&s));
    // The CHD version only matters when the setting asks.
    f.set_header_type_raw(HeaderFileType::CHD);
    assert!(f.is_deep_scanned(&s));
    let mut strict = Settings::default();
    strict.check_chd_version = true;
    assert!(!f.is_deep_scanned(&strict));
    f.chd_version = Some(5);
    assert!(f.is_deep_scanned(&strict));

    // A directory: every file in it that is there must be.
    let mut d = RvFile::new(FileType::ZIP, "z");
    let mut inside = RvFile::new(FileType::FILE_ZIP, "i");
    inside.got_status = GotStatus::GOT;
    d.children.push(inside);
    assert!(!d.is_deep_scanned(&s));
    d.children[0].got_status = GotStatus::NOT_GOT;
    assert!(d.is_deep_scanned(&s), "a missing file has nothing to scan");
}

#[test]
fn children_stay_in_name_order_and_equal_names_go_in_front() {
    let mut d = RvFile::new(FileType::DIR, "d");
    for n in ["b", "A", "c"] {
        d.child_add(RvFile::new(FileType::FILE, n));
    }
    let names: Vec<&str> = d.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["A", "b", "c"]);
    assert_eq!(d.child_name_search(FileType::FILE, "B"), (true, 1));
    // A Dir and a File of the same name are two items; the type number breaks the tie (Dir 1 <
    // File 4), so the Dir would go in front of the File.
    assert_eq!(d.child_name_search(FileType::DIR, "b"), (false, 1));
}

// ---- the database --------------------------------------------------------------------------------

fn dat(name: &str) -> RvDat {
    RvDat::new(
        1,
        Default::default(),
        vec![MetaEntry {
            id: rv_core::dat_data::DAT_ROOT_FULL_NAME,
            value: name.into(),
        }],
    )
}

/// root -> RomVault (no list) -> sub (list of two) -> game (own list of one) -> rom.
fn tree() -> RvFile {
    let mut root = RvFile::default_root();
    let mut sub = RvFile::new(FileType::DIR, "sub");
    sub.dir_dats = vec![dat("one"), dat("two")];
    let mut game = RvFile::new(FileType::DIR, "game");
    game.dat_index = Some(1);
    game.dir_dats = vec![dat("inner")];
    let mut rom = RvFile::new(FileType::FILE, "rom");
    rom.dat_index = Some(0);
    game.children.push(rom);
    let mut other = RvFile::new(FileType::DIR, "other");
    other.dat_index = Some(0);
    sub.children = vec![game, other];
    root.children[0].children.push(sub);
    root
}

#[test]
fn nodes_are_linked_to_the_nearest_ancestors_list_and_stamped_back() {
    let mut db = Database::from_root(tree()).unwrap();
    let sub = &db.root.children[0].children[0];
    let (one, two, inner) = (
        sub.dir_dats[0].uid,
        sub.dir_dats[1].uid,
        sub.children[0].dir_dats[0].uid,
    );
    assert!(one != 0 && two != 0 && inner != 0 && one != two && two != inner);
    // `game` refers into `sub`'s list (index 1); its child `rom` into `game`'s own list.
    assert_eq!(sub.children[0].dat_uid, two);
    assert_eq!(sub.children[0].children[0].dat_uid, inner);
    assert_eq!(sub.children[1].dat_uid, one);
    // DAT statuses start as "delete", like after a cache load.
    assert_eq!(db.dats.status(one), rv_core::DatUpdateStatus::Delete);
    db.dats.set_status(one, rv_core::DatUpdateStatus::Correct);
    assert_eq!(db.dats.status(one), rv_core::DatUpdateStatus::Correct);
    assert_ne!(db.dats.new_uid(), db.dats.new_uid());

    // Reorder the list: the stamp follows the DATs, not the old numbers.
    db.root.children[0].children[0].dir_dats.swap(0, 1);
    db.stamp_dat_indexes().unwrap();
    let sub = &db.root.children[0].children[0];
    assert_eq!(sub.children[0].dat_index, Some(0));
    assert_eq!(sub.children[1].dat_index, Some(1));
    assert_eq!(sub.children[0].children[0].dat_index, Some(0));
    assert_eq!(sub.dat_index, None);
}

#[test]
fn a_bad_index_or_a_dangling_dat_is_an_error_not_a_wrong_link() {
    let mut t = tree();
    t.children[0].children[0].children[1].dat_index = Some(5);
    match Database::from_root(t) {
        Err(DbError::BadDatIndex {
            index: 5,
            list_len: 2,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    let mut db = Database::from_root(tree()).unwrap();
    db.root.children[0].children[0].children[1].dat_uid = 999;
    match db.stamp_dat_indexes() {
        Err(DbError::DanglingDat { uid: 999, node }) => assert!(node.ends_with("other"), "{node}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn report_status_reset_gives_every_node_its_baseline() {
    let s = Settings::default();
    let mut db = Database::default_db();
    let mut f = RvFile::new(FileType::FILE, "f");
    f.dat_status = DatStatus::NOT_IN_DAT;
    f.got_status = GotStatus::GOT;
    f.rep_status = RepStatus::DELETE;
    f.search_found = true;
    db.root.children[0].children.push(f);
    db.report_status_reset(&s);
    let f = &db.root.children[0].children[0];
    assert_eq!((f.rep_status, f.search_found), (RepStatus::UNKNOWN, false));
    // ToSort: a Dir, InToSort, NotGot.
    assert_eq!(db.root.children[1].rep_status, RepStatus::DELETED);
}
