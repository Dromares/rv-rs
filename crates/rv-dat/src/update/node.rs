//! DAT items in the database tree: building `RvFile`s from DAT items, merging a DAT item into
//! an existing one, and the compare that decides which one it is
//! (`RomVaultCore/RvDB/RvFileDatCode.cs`, `RomVaultCore/ReadDat/DatCompare.cs`).

use rv_core::sorters::compare_name;
use rv_core::{
    Cx, EFile, FileStatus, FileType, GotStatus, HeaderFileType, MetaEntry, MiaStatus, RvFile,
    RvGame, RvTreeRow, ZipStructure,
};

use crate::{DatDir, DatFile, DatGame, DatNode};

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

/// `new RvGame(DatGame)` (`RvGame.cs:70-109`): the ids are stored ascending; an empty or
/// blank value is not stored; the categories are joined with `" | "`.
pub(crate) fn rv_game(g: &DatGame) -> RvGame {
    use rv_core::game_data as id;
    // `AddData`: insert before the first entry with an id that is not smaller; no dedupe.
    fn add(entries: &mut Vec<MetaEntry>, key: u8, value: &str, skip_blank: bool) {
        if value.is_empty() || (skip_blank && value.trim().is_empty()) {
            return;
        }
        let pos = entries.iter().take_while(|e| e.id < key).count();
        entries.insert(
            pos,
            MetaEntry {
                id: key,
                value: value.to_owned(),
            },
        );
    }
    let mut e: Vec<MetaEntry> = Vec::new();
    let mut put = |key: u8, value: &str| add(&mut e, key, value, true);
    put(id::ID, &g.id);
    put(id::DESCRIPTION, &g.description);
    put(id::CATEGORY, &g.category.join(" | "));
    put(id::ROM_OF, &g.rom_of);
    put(id::IS_BIOS, &g.is_bios);
    put(id::SOURCE_FILE, &g.source_file);
    put(id::CLONE_OF, &g.clone_of);
    put(id::CLONE_OF_ID, &g.clone_of_id);
    put(id::SAMPLE_OF, &g.sample_of);
    put(id::BOARD, &g.board);
    put(id::YEAR, &g.year);
    put(id::MANUFACTURER, &g.manufacturer);
    put(id::SERIAL, &g.serial);
    put(id::VERSION, &g.version);
    put(id::GAME_ID, &g.game_id);
    if g.is_emu_arc {
        put(id::EMU_ARC, "yes");
        put(id::PUBLISHER, &g.publisher);
        put(id::DEVELOPER, &g.developer);
        put(id::GENRE, &g.genre);
        put(id::SUB_GENRE, &g.sub_genre);
        put(id::RATINGS, &g.ratings);
        put(id::SCORE, &g.score);
        put(id::PLAYERS, &g.players);
        put(id::ENABLED, &g.enabled);
        put(id::CRC, &g.crc);
        put(id::RELATED_TO, &g.related_to);
        put(id::SOURCE, &g.source);
    }
    RvGame { entries: e }
}

fn set_dat_struct(node: &mut RvFile, dir: &DatDir) {
    node.set_zip_dat_struct(
        ZipStructure(dir.dat_struct & 0x7f),
        dir.dat_struct & 0x80 != 0,
    );
}

/// `new RvFile(DatBase, RvDat, parentHasTree)` (`RvFileDatCode.cs:62-116`). `cx` is the context
/// of the new node (its parent's tree name).
pub(crate) fn new_from_dat(
    item: &DatNode,
    dat_uid: u32,
    parent_has_tree: bool,
    cx: &Cx<'_>,
) -> RvFile {
    match item {
        DatNode::File(f) => {
            let mut n = RvFile::new(f.base.file_type, &f.base.name);
            n.size = f.size;
            n.crc = f.crc.clone();
            n.sha1 = f.sha1.clone();
            n.md5 = f.md5.clone();
            n.merge = non_empty(&f.merge);
            n.status = non_empty(&f.status);
            n.dat_uid = dat_uid;
            n.set_dat_status(f.base.dat_status, cx);
            if f.mia_status == MiaStatus::MIA_FROM_DAT {
                n.mia_set(MiaStatus::MIA_FROM_DAT);
            }
            // This could have the Required flag set on it.
            n.set_header_type_raw(f.header_file_type);
            if let Some(ts) = f.base.date_modified {
                n.file_mod_time_stamp = ts;
                n.file_status_set(FileStatus::DATE_FROM_DAT);
            }
            if n.header_type() != HeaderFileType::NOTHING {
                n.file_status_set(FileStatus::HEADER_FILE_TYPE_FROM_DAT);
            }
            if n.size.is_some() {
                n.file_status_set(FileStatus::SIZE_FROM_DAT);
            }
            if n.crc.is_some() {
                n.file_status_set(FileStatus::CRC_FROM_DAT);
            }
            if n.sha1.is_some() {
                n.file_status_set(FileStatus::SHA1_FROM_DAT);
            }
            if n.md5.is_some() {
                n.file_status_set(FileStatus::MD5_FROM_DAT);
            }
            n
        }
        DatNode::Dir(d) => {
            let mut n = RvFile::new(d.base.file_type, &d.base.name);
            if n.file_type.is_compressed_dir() {
                n.zip_struct = Some(ZipStructure::NONE);
            }
            n.dat_uid = dat_uid;
            n.set_dat_status(d.base.dat_status, cx);
            set_dat_struct(&mut n, d);
            if let Some(g) = &d.game {
                n.game = Some(rv_game(g));
            }
            if parent_has_tree && n.game.is_none() && n.tree.is_none() {
                n.tree = Some(RvTreeRow::default());
            }
            let tree = cx.tree_name(&n.name);
            let child_cx = Cx::new(cx.settings, &tree);
            let has_tree = n.tree.is_some();
            for c in &d.children {
                let child = new_from_dat(c, dat_uid, has_tree, &child_cx);
                n.child_add(child);
            }
            n
        }
    }
}

/// `RvFile.DatMergeIn` (`RvFileDatCode.cs:119-193`): attach a DAT item to an item that is
/// already in the tree. The DAT's hashes fill only what the item lacks; with `alt_file` they go
/// into the alt fields, because the file on disk has a copier header the DAT's hashes lack.
pub(crate) fn dat_merge_in(
    node: &mut RvFile,
    item: &DatNode,
    dat_uid: u32,
    alt_file: bool,
    parent_has_tree: bool,
    cx: &Cx<'_>,
    messages: &mut Vec<String>,
) {
    match item {
        DatNode::File(f) => {
            if node.header_type() == HeaderFileType::NOTHING
                && HeaderFileType(f.header_file_type.0 & HeaderFileType::HEADER_MASK.0)
                    != HeaderFileType::NOTHING
            {
                node.set_header_type_raw(f.header_file_type);
            }
            if HeaderFileType(f.header_file_type.0 & HeaderFileType::HEADER_MASK.0)
                != HeaderFileType::NOTHING
            {
                node.file_status_set(FileStatus::HEADER_FILE_TYPE_FROM_DAT);
            }
            if alt_file {
                if let Some(v) = f.size {
                    node.file_status_set(FileStatus::ALT_SIZE_FROM_DAT);
                    node.alt_size.get_or_insert(v);
                }
                if let Some(v) = &f.crc {
                    node.file_status_set(FileStatus::ALT_CRC_FROM_DAT);
                    node.alt_crc.get_or_insert_with(|| v.clone());
                }
                if let Some(v) = &f.sha1 {
                    node.file_status_set(FileStatus::ALT_SHA1_FROM_DAT);
                    node.alt_sha1.get_or_insert_with(|| v.clone());
                }
                if let Some(v) = &f.md5 {
                    node.file_status_set(FileStatus::ALT_MD5_FROM_DAT);
                    node.alt_md5.get_or_insert_with(|| v.clone());
                }
            } else {
                if let Some(v) = f.size {
                    node.file_status_set(FileStatus::SIZE_FROM_DAT);
                    node.size.get_or_insert(v);
                }
                if let Some(v) = &f.crc {
                    node.file_status_set(FileStatus::CRC_FROM_DAT);
                    node.crc.get_or_insert_with(|| v.clone());
                }
                if let Some(v) = &f.sha1 {
                    node.file_status_set(FileStatus::SHA1_FROM_DAT);
                    node.sha1.get_or_insert_with(|| v.clone());
                }
                if let Some(v) = &f.md5 {
                    node.file_status_set(FileStatus::MD5_FROM_DAT);
                    node.md5.get_or_insert_with(|| v.clone());
                }
            }
            node.merge = non_empty(&f.merge);
            node.status = non_empty(&f.status);
            if let Some(ts) = f.base.date_modified {
                node.file_mod_time_stamp = ts;
                node.file_status_set(FileStatus::DATE_FROM_DAT);
            }
            if f.mia_status == MiaStatus::MIA_FROM_DAT {
                node.mia_set(f.mia_status);
            }
        }
        DatNode::Dir(d) => {
            set_dat_struct(node, d);
            if let Some(g) = &d.game {
                node.game = Some(rv_game(g));
            }
            if parent_has_tree && node.game.is_none() && node.tree.is_none() {
                node.tree = Some(RvTreeRow::default());
            }
            if !node.dir_dats.is_empty() {
                messages.push("Setting Dir with a dat list".to_owned());
            }
        }
    }

    // Parent and time stamp should already be right.
    if node.got_status == GotStatus::NOT_GOT {
        messages.push(format!(
            "Error Adding DAT to NotGot File {}",
            node.got_status.0
        ));
    }
    node.set_dat_got_status(item.base().dat_status, node.got_status, cx);

    if node.name == item.name() {
        // The case matches, so all is good.
        node.file_name.clear();
    } else {
        node.file_name = std::mem::replace(&mut node.name, item.name().to_owned());
    }
    node.dat_uid = dat_uid;
}

/// `SetMissingStatus` (`DatUpdate.cs:671-689`): a node that came from a DAT starts out
/// `NotGot`, with nothing but what the DAT gave it.
pub(crate) fn set_missing_status(
    node: &mut RvFile,
    cx: &Cx<'_>,
    parent_got: GotStatus,
    messages: &mut Vec<String>,
) {
    if node.file_remove(cx, parent_got) == EFile::Delete {
        messages.push("Error is Set Missing Status in DatUpdate".to_owned());
        return;
    }
    if node.is_directory() {
        let tree = cx.tree_name(&node.name);
        let child_cx = Cx::new(cx.settings, &tree);
        let got = node.got_status;
        for c in &mut node.children {
            set_missing_status(c, &child_cx, got, messages);
        }
    }
}

fn eq_both(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>, found: &mut bool) -> bool {
    if let (Some(x), Some(y)) = (a, b) {
        *found = true;
        return x == y;
    }
    true
}

/// `DatCompare.CompareHash` (`DatCompare.cs:85-127`): the DAT file against the tree item.
fn compare_hash(dat: &DatFile, db: &RvFile) -> bool {
    let mut found = false;
    if let (Some(a), Some(b)) = (dat.size, db.size) {
        if a != b {
            return false;
        }
        // A zero-size DAT file with no hashes matches a zero-size file.
        if b == 0 && dat.crc.is_none() && dat.sha1.is_none() && dat.md5.is_none() {
            return true;
        }
    }
    if !eq_both(&dat.crc, &db.crc, &mut found)
        || !eq_both(&dat.sha1, &db.sha1, &mut found)
        || !eq_both(&dat.md5, &db.md5, &mut found)
    {
        return false;
    }
    found
}

/// `DatCompare.CompareAltHash` (`DatCompare.cs:130-168`): the DAT file against the hashes of
/// the item without its copier header.
fn compare_alt_hash(dat: &DatFile, db: &RvFile) -> bool {
    if !rv_core::header::alt_header_file(db.header_type()) {
        return false;
    }
    if HeaderFileType(dat.header_file_type.0 & HeaderFileType::HEADER_MASK.0) != db.header_type() {
        return false;
    }
    let mut found = false;
    if let (Some(a), Some(b)) = (dat.size, db.alt_size) {
        if a != b {
            return false;
        }
    }
    eq_both(&dat.crc, &db.alt_crc, &mut found)
        && eq_both(&dat.sha1, &db.alt_sha1, &mut found)
        && eq_both(&dat.md5, &db.alt_md5, &mut found)
        && found
}

/// `DatCompare.DatMergeCompare` (`DatCompare.cs:18-64`): is this DAT item the tree item?
/// `index_case` 0 compares names exactly, 1 case-insensitively. Returns `Some(alt_match)` for a
/// match. Directories and archives match on name and type alone.
pub fn dat_merge_compare(db: &RvFile, test: &DatNode, index_case: u8) -> Option<bool> {
    let name_equal = if index_case == 0 {
        db.name == test.name()
    } else {
        compare_name(db.file_type, &db.name, test.base().file_type, test.name()) == 0
    };
    if !name_equal {
        return None;
    }
    if db.file_type != test.base().file_type {
        return None;
    }
    let DatNode::File(dat) = test else {
        return Some(false);
    };
    if db.file_type == FileType::DIR
        || db.file_type == FileType::ZIP
        || db.file_type == FileType::SEVEN_ZIP
    {
        return Some(false);
    }
    // The header type, if the DAT requires one.
    if dat.header_file_type.0 & HeaderFileType::REQUIRED.0 != 0
        && db.header_type()
            != HeaderFileType(dat.header_file_type.0 & HeaderFileType::HEADER_MASK.0)
    {
        return None;
    }
    // A locked file cannot be compared, so it matches by name.
    if db.got_status == GotStatus::FILE_LOCKED {
        return Some(false);
    }
    if let Some(ticks) = dat.base.date_modified {
        if ticks != db.file_mod_time_stamp {
            return None;
        }
    }
    if compare_hash(dat, db) {
        return Some(false);
    }
    if compare_alt_hash(dat, db) {
        return Some(true);
    }
    None
}
