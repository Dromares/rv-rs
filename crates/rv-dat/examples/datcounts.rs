use std::path::Path;
fn walk(p: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(p).unwrap() {
        let e = e.unwrap().path();
        if e.is_dir() {
            walk(&e, out)
        } else {
            out.push(e)
        }
    }
}
fn main() {
    let root = std::env::args().nth(1).unwrap();
    let mut files = vec![];
    walk(Path::new(&root), &mut files);
    files.sort();
    for f in files {
        let t = std::time::Instant::now();
        let o = rv_dat::read_dat_file(&f);
        let h = o.header.as_ref();
        println!(
            "{:60} ok={} msgs={} games={} roms={} disks={} {:?}",
            f.file_name()
                .unwrap()
                .to_string_lossy()
                .chars()
                .take(60)
                .collect::<String>(),
            o.ok,
            o.messages.len(),
            h.map_or(0, |h| h.base_dir.game_count()),
            h.map_or(0, |h| h.base_dir.rom_count()),
            h.map_or(0, |h| h.base_dir.disk_count()),
            t.elapsed()
        );
    }
}
