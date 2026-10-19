//! Step 6: merged, split and non-merged sets (`DatSetMakeMergeSet.cs`,
//! `DatSetMakeNonMergeSet.cs`, `DatSetMakeSplitSet.cs`, `DatFindParentSets.cs`,
//! `DatHasRomOf.cs`, `DatID.cs`, `DatSetRemove.cs`).

use std::collections::HashMap;

use rv_core::{DatStatus, FileType};

use super::{dir_at, CR};
use crate::{DatDir, DatFile, DatNode};

/// `DatHasRomOf.HasRomOf` (`DatHasRomOf.cs:8-35`): is there any `romof`, `cloneof` or
/// `cloneofid`? Without one no merge step has anything to do.
pub fn has_rom_of(dir: &DatDir) -> bool {
    dir.children.iter().any(|c| {
        let DatNode::Dir(game) = c else { return false };
        match &game.game {
            None => has_rom_of(game),
            Some(g) => {
                !g.rom_of.trim().is_empty()
                    || !g.clone_of.trim().is_empty()
                    || !g.clone_of_id.trim().is_empty()
            }
        }
    })
}

/// `DatClean.DatSetMatchIDs` (`DatID.cs:25-55`): a game with a `cloneofid` gets the name of the
/// game with that id as its `cloneof`.
pub fn set_match_ids(dir: &mut DatDir) -> CR<()> {
    let mut by_id: HashMap<String, String> = HashMap::new();
    for g in 0..dir.children.len() {
        let game = dir_at(dir, g)?;
        if let Some(info) = &game.game {
            if !info.id.is_empty() {
                by_id
                    .entry(info.id.clone())
                    .or_insert_with(|| game.base.name.clone());
            }
        }
    }
    if by_id.is_empty() {
        return Ok(());
    }
    for g in 0..dir.children.len() {
        let game = dir_at(dir, g)?;
        if let Some(info) = &mut game.game {
            if !info.clone_of_id.is_empty() {
                if let Some(name) = by_id.get(&info.clone_of_id) {
                    info.clone_of = name.clone();
                }
            }
        }
    }
    Ok(())
}

/// Name and type to index of the children of a directory; the first of equal keys wins, like
/// `ChildNameSearch` on a name-sorted index.
fn child_index(dir: &DatDir) -> HashMap<(String, u8), usize> {
    let mut map = HashMap::new();
    for (i, c) in dir.children.iter().enumerate() {
        map.entry((c.name().to_owned(), c.base().file_type.0))
            .or_insert(i);
    }
    map
}

/// `DatFindParentSets.FindParentSet` (`DatFindParentSets.cs:8-37`) with `includeBios` true:
/// the chain of `romof`/`cloneof` parents of the game at `game_index`, nearest first, as
/// indices into `dir.children`.
fn find_parent_set(
    dir: &DatDir,
    index: &HashMap<(String, u8), usize>,
    game_index: usize,
    out: &mut Vec<usize>,
) {
    let DatNode::Dir(search) = &dir.children[game_index] else {
        return;
    };
    let Some(info) = &search.game else { return };
    let mut parent_name = info.rom_of.as_str();
    if parent_name.is_empty() || parent_name == search.base.name {
        parent_name = info.clone_of.as_str();
    }
    if parent_name.is_empty() || parent_name == search.base.name {
        return;
    }
    let Some(&parent) = index.get(&(parent_name.to_owned(), search.base.file_type.0)) else {
        return;
    };
    if !matches!(dir.children[parent], DatNode::Dir(_)) || out.contains(&parent) {
        return;
    }
    out.push(parent);
    find_parent_set(dir, index, parent, out);
}

fn eq_opt(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// `FindRomInParent` (`DatSetMakeMergeSet.cs:90-154`): is there a ROM in one of the parents with
/// the same size and hashes? The name is ignored. A hash or size that either side lacks does
/// not count against it.
fn find_rom_in_parent(dr0: &DatFile, parents: &[&DatDir]) -> bool {
    parents.iter().any(|game| {
        game.children.iter().any(|c| {
            let DatNode::File(dr1) = c else { return false };
            if let (Some(a), Some(b)) = (dr0.size, dr1.size) {
                if a != b {
                    return false;
                }
            }
            eq_opt(&dr0.crc, &dr1.crc)
                && eq_opt(&dr0.sha1, &dr1.sha1)
                && eq_opt(&dr0.sha256, &dr1.sha256)
                && eq_opt(&dr0.md5, &dr1.md5)
                && dr0.is_disk == dr1.is_disk
        })
    })
}

fn is_bios(g: &DatDir) -> bool {
    g.game
        .as_ref()
        .is_some_and(|i| i.is_bios.to_lowercase() == "yes")
}

/// `AlphanumComparator.CompareString` (`AlphanumComparator.cs:80-148`): natural order, digits
/// compared as numbers. The text chunks compare ordinally where ROMVault's compare is
/// culture-sensitive.
fn alphanum_compare(s1: &str, s2: &str) -> i32 {
    let a: Vec<char> = s1.chars().collect();
    let b: Vec<char> = s2.chars().collect();
    let chunk = |s: &[char], at: &mut usize| -> Vec<char> {
        let digit = |c: char| c.is_ascii_digit();
        let first = s[*at];
        let mut out = vec![first];
        *at += 1;
        while *at < s.len() && digit(s[*at]) == digit(first) {
            out.push(s[*at]);
            *at += 1;
        }
        out
    };
    let (mut m1, mut m2) = (0usize, 0usize);
    while m1 < a.len() && m2 < b.len() {
        let c1 = chunk(&a, &mut m1);
        let c2 = chunk(&b, &mut m2);
        let result = if c1[0].is_ascii_digit() && c2[0].is_ascii_digit() {
            compare_numeric(&c1, &c2)
        } else {
            let t1: String = c1.iter().collect();
            let t2: String = c2.iter().collect();
            rv_core::sorters::ordinal_compare(&t1, &t2)
        };
        if result != 0 {
            return result;
        }
    }
    a.len() as i32 - b.len() as i32
}

/// `compareTwoNumericString`: compare as numbers by padding the shorter one on the left.
fn compare_numeric(c1: &[char], c2: &[char]) -> i32 {
    let max = c1.len().max(c2.len());
    for i in 0..max {
        let x = if i + c1.len() < max {
            '0'
        } else {
            c1[i + c1.len() - max]
        };
        let y = if i + c2.len() < max {
            '0'
        } else {
            c2[i + c2.len() - max]
        };
        match x.cmp(&y) {
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Equal => {}
        }
    }
    0
}

/// `DatClean.DatSetMakeMergeSet` (`DatSetMakeMergeSet.cs:11-88`): ROMs that a BIOS parent also
/// has are dropped from the clone; the others move into the top-most non-BIOS parent (named
/// `"<game>/<rom>"` for MAME XML DATs); a clone left empty goes in `remove_empty_sets`.
pub fn make_merge_set(dir: &mut DatDir, merge_with_game_name: bool) -> CR<()> {
    // `ChildSorted(g)`, then `Array.Sort(IAlphanumComparator)`.
    let mut order: Vec<usize> = (0..dir.children.len()).collect();
    if dir.base.file_type == FileType::UNSET {
        order.sort_by(|&x, &y| {
            let (a, b) = (dir.children[x].base(), dir.children[y].base());
            rv_core::sorters::string_compare(&a.name, &b.name)
                .cmp(&0)
                .then(a.file_type.0.cmp(&b.file_type.0))
        });
    }
    for &i in &order {
        if !matches!(dir.children[i], DatNode::Dir(_)) {
            return Err(super::CleanError(format!(
                "{:?} is a file where a game was expected",
                dir.children[i].name()
            )));
        }
    }
    order
        .sort_by(|&x, &y| alphanum_compare(dir.children[x].name(), dir.children[y].name()).cmp(&0));

    let index = child_index(dir);
    for &gi in &order {
        if dir_at(dir, gi)?.game.is_none() {
            make_merge_set(dir_at(dir, gi)?, merge_with_game_name)?;
            continue;
        }
        let mut parents = Vec::new();
        find_parent_set(dir, &index, gi, &mut parents);
        if parents.is_empty() {
            continue;
        }
        let p_bios: Vec<usize> = parents
            .iter()
            .copied()
            .filter(|&p| matches!(&dir.children[p], DatNode::Dir(d) if is_bios(d)))
            .collect();
        let p_games: Vec<usize> = parents
            .iter()
            .copied()
            .filter(|p| !p_bios.contains(p))
            .collect();

        let to_test = std::mem::take(&mut dir_at(dir, gi)?.children);
        let mut keep: Vec<DatNode> = Vec::new();
        {
            let bios: Vec<&DatDir> = p_bios
                .iter()
                .filter_map(|&p| match &dir.children[p] {
                    DatNode::Dir(d) => Some(d),
                    DatNode::File(_) => None,
                })
                .collect();
            for t in to_test {
                let DatNode::File(f) = &t else {
                    keep.push(t);
                    continue;
                };
                // A no-dump ROM stays in the clone; the others go if a BIOS parent has them.
                if f.status == "nodump" || !find_rom_in_parent(f, &bios) {
                    keep.push(t);
                }
            }
        }

        if p_games.is_empty() {
            for k in keep {
                dir_at(dir, gi)?.child_add(k);
            }
            continue;
        }
        let top = *p_games.last().expect("not empty");
        let game_name = dir_at(dir, gi)?.base.name.clone();
        for mut k in keep {
            if let DatNode::File(f) = &mut k {
                if merge_with_game_name && !f.is_disk {
                    f.base.name = format!("{game_name}/{}", f.base.name);
                }
            }
            dir_at(dir, top)?.child_add(k);
        }
    }
    Ok(())
}

/// `DatClean.DatSetMakeNonMergeSet` (`DatSetMakeNonMergeSet.cs:9-83`): a game gets copies of
/// the ROMs of its devices, and of their devices, unless it has a ROM with the same SHA1 and
/// name already.
pub fn make_non_merge_set(dir: &mut DatDir) -> CR<()> {
    let index = child_index(dir);
    for g in 0..dir.children.len() {
        let game = dir_at(dir, g)?;
        if game.game.is_none() {
            make_non_merge_set(game)?;
            continue;
        }
        let refs = game
            .game
            .as_ref()
            .map(|i| i.device_ref.clone())
            .unwrap_or_default();
        if refs.is_empty() {
            continue;
        }
        let file_type = dir.base.file_type;
        let mut devices = vec![g];
        for device in &refs {
            add_device(dir, &index, file_type, device, &mut devices);
        }
        devices.remove(0);

        let mut additions: Vec<DatNode> = Vec::new();
        for &dv in &devices {
            let DatNode::Dir(device) = &dir.children[dv] else {
                continue;
            };
            for c in &device.children {
                let DatNode::File(df0) = c else { continue };
                let DatNode::Dir(game) = &dir.children[g] else {
                    continue;
                };
                let present = game.children.iter().chain(additions.iter()).any(|x| {
                    matches!(x, DatNode::File(df1)
                        if df0.sha1.is_some() && df0.sha1 == df1.sha1 && df0.base.name == df1.base.name)
                });
                if !present {
                    additions.push(c.clone());
                }
            }
        }
        let game = dir_at(dir, g)?;
        for a in additions {
            game.child_add(a);
        }
    }
    Ok(())
}

fn add_device(
    dir: &DatDir,
    index: &HashMap<(String, u8), usize>,
    file_type: FileType,
    device: &str,
    devices: &mut Vec<usize>,
) {
    let Some(&dv) = index.get(&(device.to_owned(), file_type.0)) else {
        return;
    };
    if devices.contains(&dv) {
        return;
    }
    let DatNode::Dir(d) = &dir.children[dv] else {
        return;
    };
    devices.push(dv);
    if let Some(info) = &d.game {
        for child in info.device_ref.clone() {
            add_device(dir, index, file_type, &child, devices);
        }
    }
}

/// `DatClean.DatSetMakeSplitSet` (`DatSetMakeSplitSet.cs:9-47`): a ROM that a parent (BIOS or
/// not) has stays in the clone but is marked merged.
pub fn make_split_set(dir: &mut DatDir) -> CR<()> {
    let index = child_index(dir);
    for g in 0..dir.children.len() {
        if dir_at(dir, g)?.game.is_none() {
            make_split_set(dir_at(dir, g)?)?;
            continue;
        }
        let mut parents = Vec::new();
        find_parent_set(dir, &index, g, &mut parents);
        if parents.is_empty() {
            continue;
        }
        let found: Vec<bool> = {
            let parent_dirs: Vec<&DatDir> = parents
                .iter()
                .filter_map(|&p| match &dir.children[p] {
                    DatNode::Dir(d) => Some(d),
                    DatNode::File(_) => None,
                })
                .collect();
            let DatNode::Dir(game) = &dir.children[g] else {
                continue;
            };
            game.children
                .iter()
                .map(|c| match c {
                    DatNode::File(f) => f.status != "nodump" && find_rom_in_parent(f, &parent_dirs),
                    DatNode::Dir(_) => false,
                })
                .collect()
        };
        let game = dir_at(dir, g)?;
        for (c, merged) in game.children.iter_mut().zip(found) {
            if let (DatNode::File(f), true) = (c, merged) {
                if f.merge.is_empty() {
                    f.merge = "(Auto Merged)".to_owned();
                }
                f.base.dat_status = DatStatus::IN_DAT_MERGED;
            }
        }
    }
    Ok(())
}

/// `DatClean.RemoveDeviceRef` (`DatClean.cs:166-182`).
pub fn remove_device_ref(dir: &mut DatDir) {
    for c in &mut dir.children {
        if let DatNode::Dir(d) = c {
            if let Some(g) = &mut d.game {
                g.device_ref.clear();
            }
            remove_device_ref(d);
        }
    }
}

/// `DatClean.RemoveDupes` (`DatSetRemove.cs:11-99`): inside a game, drop a ROM that another
/// one duplicates (same hashes; same name unless `test_name` is off; a name with `/` loses to
/// one without). `test_with_merge_name` also treats a ROM whose name is another's merge name as
/// a duplicate.
pub fn remove_dupes(dir: &mut DatDir, test_name: bool, test_with_merge_name: bool) -> CR<()> {
    for g in 0..dir.children.len() {
        let game = dir_at(dir, g)?;
        if game.game.is_none() {
            // ROMVault passes only `testName` here, so `testWithMergeName` is off below.
            remove_dupes(game, test_name, false)?;
            continue;
        }
        loop {
            let mut found = false;
            'scan: for r in 0..game.children.len() {
                for t in (r + 1)..game.children.len() {
                    let (DatNode::File(df0), DatNode::File(df1)) =
                        (&game.children[r], &game.children[t])
                    else {
                        return Err(super::CleanError(format!(
                            "{:?} holds a directory where a ROM was expected",
                            game.base.name
                        )));
                    };
                    if test_name && df0.base.name != df1.base.name {
                        continue;
                    }
                    let has_crc = df0.crc.is_some() && df1.crc.is_some();
                    if has_crc && df0.crc != df1.crc {
                        continue;
                    }
                    let has_sha1 = df0.sha1.is_some() && df1.sha1.is_some();
                    if has_sha1 && df0.sha1 != df1.sha1 {
                        continue;
                    }
                    let has_sha256 = df0.sha256.is_some() && df1.sha256.is_some();
                    if has_sha256 && df0.sha256 != df1.sha256 {
                        continue;
                    }
                    let has_md5 = df0.md5.is_some() && df1.md5.is_some();
                    if has_md5 && df0.md5 != df1.md5 {
                        continue;
                    }
                    if !has_crc && !has_sha1 && !has_md5 {
                        continue;
                    }

                    let (name0, name1) = (df0.base.name.as_str(), df1.base.name.as_str());
                    let (n0, n1) = (name0.contains('/'), name1.contains('/'));
                    let remove = if n0 && !n1 {
                        Some(r)
                    } else if !n0 && n1 {
                        Some(t)
                    } else if n0 && n1 {
                        let s0 = &name0[..name0.find('/').expect("contains")];
                        let s1 = &name1[..name1.find('/').expect("contains")];
                        if s0 != s1 {
                            Some(t)
                        } else if alphanum_compare(name0, name1) >= 0 {
                            Some(r)
                        } else {
                            Some(t)
                        }
                    } else if name0 == name1 || (test_with_merge_name && name0 == df1.merge) {
                        Some(t)
                    } else {
                        None
                    };
                    if let Some(i) = remove {
                        game.children.remove(i);
                        found = true;
                        break 'scan;
                    }
                }
            }
            if !found {
                break;
            }
        }
    }
    Ok(())
}

/// `DatClean.RemoveEmptySets` (`DatSetRemove.cs:103-130`): directories that end up with no ROM
/// anywhere below them are dropped. The root stays.
pub fn remove_empty_sets(dir: &mut DatDir) -> bool {
    fn keep(node: &mut DatNode) -> bool {
        match node {
            DatNode::File(_) => true,
            DatNode::Dir(d) => {
                if d.children.is_empty() {
                    return false;
                }
                let old = std::mem::take(&mut d.children);
                let mut found = false;
                for mut c in old {
                    if keep(&mut c) {
                        found = true;
                        d.child_add(c);
                    }
                }
                found
            }
        }
    }
    let old = std::mem::take(&mut dir.children);
    let mut found = false;
    for mut c in old {
        if keep(&mut c) {
            found = true;
            dir.child_add(c);
        }
    }
    found
}
