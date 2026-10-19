//! Parse a cache, print what is in it and optionally write it back out.
//!
//! ```sh
//! cargo run -p rv-cache --example cacheprobe -- IN.Cache [OUT.Cache]
//! ```
//!
//! `OUT` is written with the plain `fs::write`, not the crash-safe save, so it can be
//! compared with `cmp` and `sha256sum` against `IN`. Never point `OUT` at a real cache.

use std::collections::BTreeMap;
use std::process::ExitCode;

use rv_cache::CacheFile;
use rv_core::RvFile;

#[derive(Default)]
struct Stats {
    file_types: BTreeMap<u8, usize>,
    dat_status: BTreeMap<u8, usize>,
    unknown_flag_nodes: usize,
    dir_dat_lists: usize,
    dats: usize,
    games: usize,
    tree_rows: usize,
    legacy_sort_bits: usize,
    max_depth: usize,
}

fn walk(node: &RvFile, depth: usize, stats: &mut Stats) {
    *stats.file_types.entry(node.file_type.0).or_default() += 1;
    *stats.dat_status.entry(node.dat_status.0).or_default() += 1;
    stats.unknown_flag_nodes += usize::from(node.unknown_flag_bits != 0);
    stats.dir_dat_lists += usize::from(!node.dir_dats.is_empty());
    stats.dats += node.dir_dats.len();
    stats.games += usize::from(node.game.is_some());
    stats.tree_rows += usize::from(node.tree.is_some());
    stats.legacy_sort_bits += usize::from(node.file_status.0 >> 30 != 0);
    stats.max_depth = stats.max_depth.max(depth);
    for child in &node.children {
        walk(child, depth + 1, stats);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else {
        eprintln!("usage: cacheprobe IN.Cache [OUT.Cache]");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cache = match CacheFile::from_bytes(&bytes) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{input}: rejected: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stats = Stats::default();
    walk(&cache.root, 0, &mut stats);
    println!("file: {input}");
    println!("size: {} bytes", bytes.len());
    println!("version: {}", cache.version);
    println!("nodes: {}", cache.root.node_count());
    println!("root children: {}", cache.root.children.len());
    println!("file types (raw u8: count): {:?}", stats.file_types);
    println!("dat status (raw u8: count): {:?}", stats.dat_status);
    println!(
        "nodes with undefined flag bits 21-31: {}",
        stats.unknown_flag_nodes
    );
    println!(
        "nodes with legacy FileStatus bits 30/31: {}",
        stats.legacy_sort_bits
    );
    println!(
        "dir-dat lists: {}, dats: {}, games: {}, tree rows: {}",
        stats.dir_dat_lists, stats.dats, stats.games, stats.tree_rows
    );
    println!("max depth: {}", stats.max_depth);
    if let Some(output) = args.get(1) {
        let out = match cache.to_bytes() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("serialize: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(e) = std::fs::write(output, out) {
            eprintln!("{output}: {e}");
            return ExitCode::FAILURE;
        }
        println!("wrote: {output}");
    }
    ExitCode::SUCCESS
}
