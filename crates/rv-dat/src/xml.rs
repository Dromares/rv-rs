//! XML DATs: `<datafile>` / RVDatFile (`DatXMLReader.cs`), MAME `<mame>`/`<mess>`, and the
//! MAME/MESS `<softwarelist>` (`DatMessXmlReader.cs`).

use rv_core::FileType;

use crate::error::{Abort, R};
use crate::model::{DatDir, DatFile, DatGame, DatHeader, DatNode};
use crate::varfix::{clean_chd, clean_hash, to_lower, ulong};
use crate::xmldom::Element;

fn hash_attr(e: &Element, name: &str, len: usize) -> Option<Vec<u8>> {
    // `CleanMD5SHA1(node, length)` uses the attribute's text, null when the attribute is absent.
    e.attr(name).and_then(|v| clean_hash(v, len))
}

// ---------------------------------------------------------------------------------------
// <datafile>
// ---------------------------------------------------------------------------------------

/// `DatXmlReader.ReadDat` (`DatXMLReader.cs:13-73`). The root has a `<header>` child.
pub(crate) fn read_datafile(root: &Element, filename: &str, h: &mut DatHeader) -> R<bool> {
    if !load_header(root, filename, h) {
        return Ok(false);
    }
    // Direct children of the root only, grouped by element name in this fixed order
    // (`:27-70`), not in document order.
    for dir in root.select("dir") {
        load_dir(&mut h.base_dir, dir);
    }
    for game in root.select("game") {
        load_game(&mut h.base_dir, game);
    }
    for machine in root.select("machine") {
        load_game(&mut h.base_dir, machine);
    }
    for rom in root.select("rom") {
        load_rom(&mut h.base_dir, rom);
    }
    for disk in root.select("disk") {
        load_disk(&mut h.base_dir, disk);
    }
    Ok(true)
}

/// `DatXmlReader.ReadMameDat` (`DatXMLReader.cs:75-121`): root `<mame>` or `<mess>`.
pub(crate) fn read_mame(root: &Element, filename: &str, h: &mut DatHeader) -> R<bool> {
    h.mame_xml = true;
    // `LoadMameHeaderFromDat` (`:176-198`)
    h.filename = filename.to_owned();
    h.name = root.attr_str("build").to_owned();
    h.description = root.attr_str("build").to_owned();
    h.version = root.attr_str("version").to_owned();
    h.date = root.attr_str("date").to_owned();
    for dir in root.select("dir") {
        load_dir(&mut h.base_dir, dir);
    }
    for game in root.select("game") {
        load_game(&mut h.base_dir, game);
    }
    for machine in root.select("machine") {
        load_game(&mut h.base_dir, machine);
    }
    Ok(true)
}

/// `LoadHeaderFromDat` (`:124-172`). False when there is no `<header>`.
fn load_header(root: &Element, filename: &str, h: &mut DatHeader) -> bool {
    h.filename = filename.to_owned();
    let Some(head) = root.select_one("header") else {
        return false;
    };
    h.id = head.child_text("id");
    h.name = head.child_text("name");
    h.r#type = head.child_text("type");
    h.root_dir = head.child_text("rootdir");
    h.description = head.child_text("description");
    h.subset = head.child_text("subset");
    h.category = head.child_text("category");
    h.version = head.child_text("version");
    h.date = head.child_text("date");
    h.author = head.child_text("author");
    h.email = head.child_text("email");
    h.homepage = head.child_text("homepage");
    h.url = head.child_text("url");
    h.comment = head.child_text("comment");
    // `<romvault>` is preferred over `<clrmamepro>`.
    if let Some(packing) = head
        .select_one("romvault")
        .or_else(|| head.select_one("clrmamepro"))
    {
        h.header = packing.attr_str("header").to_owned();
        h.compression = to_lower(packing.attr_str("forcepacking"));
        h.merge_type = to_lower(packing.attr_str("forcemerging"));
        h.dir = to_lower(packing.attr_str("dir"));
    }
    let nz = head.child_text("notzipped");
    h.not_zipped = nz.eq_ignore_ascii_case("true") || nz.eq_ignore_ascii_case("yes");
    true
}

/// `LoadDirFromDat` (`:201-239`).
fn load_dir(parent: &mut DatDir, node: &Element) {
    let mut dir = DatDir::new(node.attr_str("name"), FileType::UNSET);
    for d in node.select("dir") {
        load_dir(&mut dir, d);
    }
    for g in node.select("game") {
        load_game(&mut dir, g);
    }
    for m in node.select("machine") {
        load_game(&mut dir, m);
    }
    parent.child_add(DatNode::Dir(dir));
}

/// `LoadGameFromDat` (`:241-341`).
fn load_game(parent: &mut DatDir, node: &Element) {
    let file_type = match to_lower(node.attr_str("type")).as_str() {
        "dir" => FileType::DIR,
        "zip" => FileType::ZIP,
        "7z" => FileType::SEVEN_ZIP,
        _ => FileType::UNSET,
    };
    let mut dir = DatDir::new(node.attr_str("name"), file_type);
    let mut g = DatGame {
        id: node.attr_str("id").to_owned(),
        rom_of: node.attr_str("romof").to_owned(),
        clone_of: node.attr_str("cloneof").to_owned(),
        clone_of_id: node.attr_str("cloneofid").to_owned(),
        sample_of: node.attr_str("sampleof").to_owned(),
        description: node.child_text("description"),
        source_file: node.attr_str("sourcefile").to_owned(),
        is_bios: node.attr_str("isbios").to_owned(),
        is_device: node.attr_str("isdevice").to_owned(),
        board: node.attr_str("board").to_owned(),
        year: node.child_text("year"),
        manufacturer: node.child_text("manufacturer"),
        runnable: node.attr_str("runnable").to_owned(),
        serial: node.child_text("serial"),
        version: node.child_text("version"),
        game_id: node.child_text("game_id"),
        ..DatGame::default()
    };
    // EmuArc / TruRip overrides (`:283-301`).
    if let Some(e) = node
        .select_one("tea")
        .or_else(|| node.select_one("trurip"))
        .or_else(|| node.select_one("EmuArc"))
    {
        g.is_emu_arc = true;
        g.id = e.child_text("titleid");
        g.publisher = e.child_text("publisher");
        g.developer = e.child_text("developer");
        g.year = e.child_text("year");
        g.genre = e.child_text("genre");
        g.sub_genre = e.child_text("subgenre");
        g.ratings = e.child_text("ratings");
        g.score = e.child_text("score");
        g.players = e.child_text("players");
        g.enabled = e.child_text("enabled");
        g.crc = e.child_text("crc");
        g.clone_of = e.child_text("cloneof");
        g.related_to = e.child_text("relatedto");
        g.source = e.child_text("source");
    }
    for r in node.select("rom") {
        load_rom(&mut dir, r);
    }
    for d in node.select("disk") {
        load_disk(&mut dir, d);
    }
    for c in node.select("category") {
        let text = c.inner_text();
        if !text.is_empty() {
            g.category.push(text);
        }
    }
    for d in node.select("device_ref") {
        let name = d.attr_str("name");
        if !name.trim().is_empty() {
            g.device_ref.push(name.to_owned());
        }
    }
    dir.game = Some(Box::new(g));
    parent.child_add(DatNode::Dir(dir));
}

/// `LoadRomFromDat` (`:344-366`). The `date` attribute is commented out in the source
/// (`:361`), so XML DATs never carry file dates.
fn load_rom(parent: &mut DatDir, node: &Element) {
    let mut rom = DatFile::new(node.attr_str("name"));
    rom.size = node.attr("size").and_then(ulong);
    rom.crc = hash_attr(node, "crc", 8);
    rom.sha1 = hash_attr(node, "sha1", 40);
    rom.sha256 = hash_attr(node, "sha256", 64);
    rom.md5 = hash_attr(node, "md5", 32);
    rom.merge = node.attr_str("merge").to_owned();
    rom.status = to_lower(node.attr_str("status"));
    rom.region = to_lower(node.attr_str("region"));
    rom.mia = node.attr_str("mia").to_owned();
    parent.child_add(DatNode::File(rom));
}

/// `LoadDiskFromDat` (`:368-387`).
fn load_disk(parent: &mut DatDir, node: &Element) {
    let mut rom = DatFile::new(&clean_chd(node.attr_str("name")));
    rom.sha1 = hash_attr(node, "sha1", 40);
    rom.sha256 = hash_attr(node, "sha256", 64);
    rom.md5 = hash_attr(node, "md5", 32);
    rom.merge = clean_chd(node.attr_str("merge"));
    rom.status = to_lower(node.attr_str("status"));
    rom.mia = node.attr_str("mia").to_owned();
    rom.is_disk = true;
    parent.child_add(DatNode::File(rom));
}

// ---------------------------------------------------------------------------------------
// <softwarelist>
// ---------------------------------------------------------------------------------------

/// `DatMessXmlReader.ReadDat` (`DatMessXmlReader.cs:10-29`). `root` is the document element
/// if it is `<softwarelist>`, else `None` (the header lookup then fails, `:39-42`).
pub(crate) fn read_softwarelist(
    root: Option<&Element>,
    filename: &str,
    h: &mut DatHeader,
) -> R<bool> {
    let Some(root) = root else {
        return Ok(false);
    };
    h.filename = filename.to_owned();
    h.name = root.attr_str("name").to_owned();
    h.description = root.attr_str("description").to_owned();
    for software in root.select("software") {
        load_software(&mut h.base_dir, software)?;
    }
    Ok(true)
}

/// `LoadGameFromDat` (`DatMessXmlReader.cs:56-128`).
fn load_software(parent: &mut DatDir, node: &Element) -> R<()> {
    let mut dir = DatDir::new(node.attr_str("name"), FileType::UNSET);
    dir.game = Some(Box::new(DatGame {
        description: node.child_text("description"),
        rom_of: node.attr_str("romof").to_owned(),
        clone_of: node.attr_str("cloneof").to_owned(),
        year: node.child_text("year"),
        manufacturer: node.child_text("publisher"),
        ..DatGame::default()
    }));
    let parts: Vec<&Element> = node.select("part").collect();
    for part in &parts {
        let mut index_continue: Option<usize> = None;
        for area in part.select("dataarea") {
            for rom in area.select("rom") {
                load_software_rom(&mut dir, rom, &mut index_continue)?;
            }
        }
    }
    for part in &parts {
        for area in part.select("diskarea") {
            for disk in area.select("disk") {
                let mut f = DatFile::new(&clean_chd(disk.attr_str("name")));
                f.sha1 = hash_attr(disk, "sha1", 40);
                f.status = to_lower(disk.attr_str("status"));
                f.is_disk = true;
                dir.child_add(DatNode::File(f));
            }
        }
    }
    if !dir.children.is_empty() {
        parent.child_add(DatNode::Dir(dir));
    }
    Ok(())
}

/// `LoadRomFromDat` (`DatMessXmlReader.cs:130-161`).
fn load_software_rom(
    parent: &mut DatDir,
    node: &Element,
    index_continue: &mut Option<usize>,
) -> R<()> {
    let loadflag = node.attr_str("loadflag");
    if let Some(name) = node.attr("name") {
        let mut rom = DatFile::new(name);
        rom.size = node.attr("size").and_then(ulong);
        rom.crc = hash_attr(node, "crc", 8);
        rom.sha1 = hash_attr(node, "sha1", 40);
        rom.status = to_lower(node.attr_str("status"));
        *index_continue = Some(parent.child_add(DatNode::File(rom)));
    } else if loadflag.eq_ignore_ascii_case("continue") || loadflag.eq_ignore_ascii_case("ignore") {
        // `parentDir[indexContinue]` with -1 throws; `Size += ULong(...)` is null when either
        // side is null.
        let Some(i) = *index_continue else {
            return Err(Abort(
                "Index was out of range. Must be non-negative and less than the size of the collection."
                    .into(),
            ));
        };
        let add = node.attr("size").and_then(ulong);
        if let DatNode::File(f) = &mut parent.children[i] {
            f.size = match (f.size, add) {
                (Some(a), Some(b)) => Some(a.wrapping_add(b)),
                _ => None,
            };
        }
    }
    Ok(())
}
