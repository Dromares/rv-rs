//! Steps 1 to 5, 7 and 8: names, filters, ids, categories and statuses
//! (`DatSetCleanFilenames.cs`, `DatSetRemove.cs`, `DatID.cs`, `DatClean.cs`, `DatSetStatus.cs`).

use rv_core::{DatStatus, MiaStatus};

use super::{dir_at, CR};
use crate::{DatDir, DatFile, DatNode};

/// `DatClean.CleanFilenames` (`DatSetCleanFilenames.cs:62-97`, step 1): `\` and `./` become
/// `/`, and the characters `: * ? < > | "` and control characters become `-`.
pub fn clean_filenames(dir: &mut DatDir) {
    for child in &mut dir.children {
        match child {
            DatNode::Dir(d) => {
                clean_filename(&mut d.base.name);
                clean_filenames(d);
            }
            DatNode::File(f) => clean_filename(&mut f.base.name),
        }
    }
}

fn clean_filename(name: &mut String) {
    if name.is_empty() {
        return;
    }
    let replaced = name.replace('\\', "/").replace("./", "/");
    *name = replaced
        .chars()
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '<' | '>' | '|' | '"') || (c as u32) < 32 {
                '-'
            } else {
                c
            }
        })
        .collect();
}

fn retain_files(dir: &mut DatDir, keep: impl Fn(&DatFile) -> bool + Copy) -> CR<()> {
    for g in 0..dir.children.len() {
        let game = dir_at(dir, g)?;
        if game.game.is_none() {
            retain_files(game, keep)?;
        } else {
            game.children.retain(|c| match c {
                DatNode::File(f) => keep(f),
                DatNode::Dir(_) => true,
            });
        }
    }
    Ok(())
}

/// `DatClean.RemoveCHD` (`DatSetRemove.cs:188-212`): the `RomsOnly` filter drops every disk.
pub fn remove_chd(dir: &mut DatDir) -> CR<()> {
    retain_files(dir, |f| !f.is_disk)
}

/// `DatClean.RemoveNonCHD` (`DatSetRemove.cs:213-235`): the `CHDsOnly` filter keeps only disks.
pub fn remove_non_chd(dir: &mut DatDir) -> CR<()> {
    retain_files(dir, |f| f.is_disk)
}

/// `DatClean.RemoveNoDumps` (`DatSetRemove.cs:162-186`, step 3): every ROM whose status is
/// `nodump` goes, so `InDatNoDump` never reaches the database.
pub fn remove_no_dumps(dir: &mut DatDir) {
    for child in &mut dir.children {
        let DatNode::Dir(game) = child else { continue };
        if game.game.is_none() {
            remove_no_dumps(game);
        } else {
            game.children
                .retain(|c| !matches!(c, DatNode::File(f) if f.status == "nodump"));
        }
    }
}

/// `DatClean.DatSetAddIdNumbers` (`DatID.cs:10-23`, step 4): every item, ROM files included,
/// is renamed `"<Id> - <name>"`. The id is the nearest game's, and an item without one inherits
/// the id of the sibling before it.
pub fn set_add_id_numbers(dir: &mut DatDir, id: &str) {
    let mut id = id.to_owned();
    for child in &mut dir.children {
        if let DatNode::Dir(d) = child {
            if let Some(game) = &d.game {
                if !game.id.is_empty() {
                    id = game.id.clone();
                }
            }
        }
        match child {
            DatNode::Dir(d) => {
                d.base.name = format!("{id} - {}", d.base.name);
                set_add_id_numbers(d, &id);
            }
            DatNode::File(f) => f.base.name = format!("{id} - {}", f.base.name),
        }
    }
}

/// `DatClean.AddCategory` (`DatClean.cs:258-282`, step 5): `"<category>/<game>"`.
pub fn add_category(dir: &mut DatDir, order: &[String]) {
    for child in &mut dir.children {
        let DatNode::Dir(game) = child else { continue };
        if game.game.is_none() {
            add_category(game, order);
            continue;
        }
        if let Some(cat) = find_category(game, order) {
            game.base.name = format!("{cat}/{}", game.base.name);
        }
    }
}

/// `DatClean.FindCategory` (`DatClean.cs:284-311`): a single category is used as it is, else
/// the best-ranked one in `order` (case-insensitive), else none.
pub fn find_category(game: &DatDir, order: &[String]) -> Option<String> {
    let cats = &game.game.as_ref()?.category;
    if cats.is_empty() {
        return None;
    }
    if cats.len() == 1 && !cats[0].trim().is_empty() {
        return Some(cats[0].clone());
    }
    let mut best: Option<usize> = None;
    for cat in cats {
        if cat.trim().is_empty() {
            continue;
        }
        if let Some(i) = order
            .iter()
            .position(|o| o.to_lowercase() == cat.to_lowercase())
        {
            if best.is_none_or(|b| i < b) {
                best = Some(i);
            }
        }
    }
    best.map(|i| order[i].clone())
}

/// `DatSetStatus.SetStatus` (`DatSetStatus.cs:9-60`, step 7): the MIA flag, the `(No-Merge)`
/// prefix on merge names, and the `DatStatus` of every ROM.
pub fn set_status(dir: &mut DatDir) {
    for child in &mut dir.children {
        match child {
            DatNode::Dir(d) => set_status(d),
            DatNode::File(f) => rom_check_collect(f),
        }
    }
}

fn rom_check_collect(rom: &mut DatFile) {
    rom.mia_status = MiaStatus::NONE;
    if rom.base.dat_status == DatStatus::IN_DAT_MERGED {
        return;
    }
    // `tRom.Size != 0`: a ROM without a size counts too.
    if rom.mia.to_lowercase() == "yes" && rom.size != Some(0) {
        rom.mia_status = MiaStatus::MIA_FROM_DAT;
    }
    if !rom.merge.is_empty() {
        rom.merge = format!("(No-Merge) {}", rom.merge);
    }
    if rom.status == "nodump" {
        rom.base.dat_status = DatStatus::IN_DAT_NO_DUMP;
        return;
    }
    rom.base.dat_status = DatStatus::IN_DAT_COLLECT;
}

/// `DatClean.CheckDeDuped` (`DatClean.cs:184-203`, step 8): `deduped` ROMs are merged ones.
pub fn check_de_duped(dir: &mut DatDir) {
    for child in &mut dir.children {
        match child {
            DatNode::File(f) if f.status.to_lowercase() == "deduped" => {
                f.base.dat_status = DatStatus::IN_DAT_MERGED
            }
            DatNode::File(_) => {}
            DatNode::Dir(d) => check_de_duped(d),
        }
    }
}
