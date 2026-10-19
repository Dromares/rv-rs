//! `RepairStatus.StatusCheck`: every coded `(FileType, DatStatus, GotStatus)` triple, and every
//! one that is not coded. The expected rows below are the 78 `StatusCheck[...] = new List<RepStatus>`
//! assignments of `RepairStatus.InitStatusCheck` (`RepairStatus.cs:80-178`), in source order; the
//! first entry of each list is the baseline status.

use rv_core::{baseline_status, status_check, DatStatus, FileType, GotStatus, RepStatus};

type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
);

const CODED: &[Row] = &[
    ("Dir", "InDatCollect", "NotGot", &["DirMissing"]),
    ("Dir", "InDatCollect", "Got", &["DirCorrect"]),
    ("Dir", "InToSort", "NotGot", &["Deleted"]),
    ("Dir", "InToSort", "Got", &["DirInToSort"]),
    ("Dir", "NotInDat", "NotGot", &["Deleted"]),
    ("Dir", "NotInDat", "Got", &["DirUnknown"]),
    ("Zip", "InDatCollect", "NotGot", &["DirMissing"]),
    ("Zip", "InDatCollect", "Got", &["DirCorrect"]),
    ("Zip", "InDatCollect", "Corrupt", &["DirCorrupt"]),
    ("Zip", "InDatCollect", "FileLocked", &["UnScanned"]),
    ("Zip", "InToSort", "NotGot", &["Deleted"]),
    ("Zip", "InToSort", "Got", &["DirInToSort"]),
    ("Zip", "InToSort", "Corrupt", &["DirCorrupt"]),
    ("Zip", "InToSort", "FileLocked", &["UnScanned"]),
    ("Zip", "NotInDat", "NotGot", &["Deleted"]),
    ("Zip", "NotInDat", "Got", &["DirUnknown"]),
    ("Zip", "NotInDat", "Corrupt", &["DirCorrupt"]),
    ("Zip", "NotInDat", "FileLocked", &["UnScanned"]),
    ("SevenZip", "InDatCollect", "NotGot", &["DirMissing"]),
    ("SevenZip", "InDatCollect", "Got", &["DirCorrect"]),
    ("SevenZip", "InDatCollect", "Corrupt", &["DirCorrupt"]),
    ("SevenZip", "InDatCollect", "FileLocked", &["UnScanned"]),
    ("SevenZip", "InToSort", "NotGot", &["Deleted"]),
    ("SevenZip", "InToSort", "Got", &["DirInToSort"]),
    ("SevenZip", "InToSort", "Corrupt", &["DirCorrupt"]),
    ("SevenZip", "InToSort", "FileLocked", &["UnScanned"]),
    ("SevenZip", "NotInDat", "NotGot", &["Deleted"]),
    ("SevenZip", "NotInDat", "Got", &["DirUnknown"]),
    ("SevenZip", "NotInDat", "Corrupt", &["DirCorrupt"]),
    ("SevenZip", "NotInDat", "FileLocked", &["UnScanned"]),
    (
        "File",
        "InDatCollect",
        "NotGot",
        &["Missing", "CanBeFixed", "Incomplete"],
    ),
    (
        "File",
        "InDatCollect",
        "Got",
        &[
            "Correct",
            "IncompleteRemove",
            "MoveToSort",
            "NeededForFix",
            "Delete",
        ],
    ),
    (
        "File",
        "InDatCollect",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "CorruptCanBeFixed"],
    ),
    ("File", "InDatCollect", "FileLocked", &["UnScanned"]),
    ("File", "InDatMerged", "NotGot", &["NotCollected"]),
    (
        "File",
        "InDatMerged",
        "Got",
        &["UnNeeded", "Delete", "MoveToSort", "NeededForFix"],
    ),
    (
        "File",
        "InDatMerged",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "Delete"],
    ),
    ("File", "InDatMerged", "FileLocked", &["UnScanned"]),
    ("File", "InToSort", "NotGot", &["Deleted"]),
    (
        "File",
        "InToSort",
        "Got",
        &["InToSort", "Ignore", "NeededForFix", "Delete"],
    ),
    ("File", "InToSort", "Corrupt", &["Corrupt", "Delete"]),
    ("File", "InToSort", "FileLocked", &["UnScanned"]),
    ("File", "NotInDat", "NotGot", &["Deleted"]),
    (
        "File",
        "NotInDat",
        "Got",
        &[
            "Unknown",
            "Ignore",
            "Delete",
            "MoveToSort",
            "NeededForFix",
            "Rename",
        ],
    ),
    (
        "File",
        "NotInDat",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "Delete"],
    ),
    ("File", "NotInDat", "FileLocked", &["UnScanned"]),
    (
        "FileZip",
        "InDatCollect",
        "NotGot",
        &["Missing", "CanBeFixed", "Incomplete"],
    ),
    (
        "FileZip",
        "InDatCollect",
        "Got",
        &[
            "Correct",
            "IncompleteRemove",
            "MoveToSort",
            "NeededForFix",
            "Delete",
        ],
    ),
    (
        "FileZip",
        "InDatCollect",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "CorruptCanBeFixed"],
    ),
    ("FileZip", "InDatCollect", "FileLocked", &["UnScanned"]),
    ("FileZip", "InDatMerged", "NotGot", &["NotCollected"]),
    (
        "FileZip",
        "InDatMerged",
        "Got",
        &["UnNeeded", "Delete", "MoveToSort", "NeededForFix", "Rename"],
    ),
    (
        "FileZip",
        "InDatMerged",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "Delete"],
    ),
    ("FileZip", "InDatMerged", "FileLocked", &["UnScanned"]),
    ("FileZip", "InToSort", "NotGot", &["Deleted"]),
    (
        "FileZip",
        "InToSort",
        "Got",
        &["InToSort", "NeededForFix", "Delete"],
    ),
    ("FileZip", "InToSort", "Corrupt", &["Corrupt", "Delete"]),
    ("FileZip", "InToSort", "FileLocked", &["UnScanned"]),
    ("FileZip", "NotInDat", "NotGot", &["Deleted"]),
    (
        "FileZip",
        "NotInDat",
        "Got",
        &["Unknown", "Delete", "MoveToSort", "NeededForFix", "Rename"],
    ),
    (
        "FileZip",
        "NotInDat",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "Delete"],
    ),
    ("FileZip", "NotInDat", "FileLocked", &["UnScanned"]),
    (
        "FileSevenZip",
        "InDatCollect",
        "NotGot",
        &["Missing", "CanBeFixed", "Incomplete"],
    ),
    (
        "FileSevenZip",
        "InDatCollect",
        "Got",
        &[
            "Correct",
            "IncompleteRemove",
            "MoveToSort",
            "NeededForFix",
            "Delete",
        ],
    ),
    (
        "FileSevenZip",
        "InDatCollect",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "CorruptCanBeFixed"],
    ),
    ("FileSevenZip", "InDatCollect", "FileLocked", &["UnScanned"]),
    ("FileSevenZip", "InDatMerged", "NotGot", &["NotCollected"]),
    (
        "FileSevenZip",
        "InDatMerged",
        "Got",
        &["UnNeeded", "Delete", "MoveToSort", "NeededForFix", "Rename"],
    ),
    (
        "FileSevenZip",
        "InDatMerged",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "Delete"],
    ),
    ("FileSevenZip", "InDatMerged", "FileLocked", &["UnScanned"]),
    ("FileSevenZip", "InToSort", "NotGot", &["Deleted"]),
    (
        "FileSevenZip",
        "InToSort",
        "Got",
        &["InToSort", "NeededForFix", "Delete"],
    ),
    (
        "FileSevenZip",
        "InToSort",
        "Corrupt",
        &["Corrupt", "Delete"],
    ),
    ("FileSevenZip", "InToSort", "FileLocked", &["UnScanned"]),
    ("FileSevenZip", "NotInDat", "NotGot", &["Deleted"]),
    (
        "FileSevenZip",
        "NotInDat",
        "Got",
        &["Unknown", "Delete", "MoveToSort", "NeededForFix", "Rename"],
    ),
    (
        "FileSevenZip",
        "NotInDat",
        "Corrupt",
        &["Corrupt", "MoveToCorrupt", "Delete"],
    ),
    ("FileSevenZip", "NotInDat", "FileLocked", &["UnScanned"]),
];

fn file_type(name: &str) -> FileType {
    match name {
        "Dir" => FileType::DIR,
        "Zip" => FileType::ZIP,
        "SevenZip" => FileType::SEVEN_ZIP,
        "File" => FileType::FILE,
        "FileZip" => FileType::FILE_ZIP,
        "FileSevenZip" => FileType::FILE_SEVEN_ZIP,
        other => panic!("{other}"),
    }
}

fn dat_status(name: &str) -> DatStatus {
    match name {
        "InDatCollect" => DatStatus::IN_DAT_COLLECT,
        "InDatMerged" => DatStatus::IN_DAT_MERGED,
        "InToSort" => DatStatus::IN_TO_SORT,
        "NotInDat" => DatStatus::NOT_IN_DAT,
        other => panic!("{other}"),
    }
}

fn got_status(name: &str) -> GotStatus {
    match name {
        "NotGot" => GotStatus::NOT_GOT,
        "Got" => GotStatus::GOT,
        "Corrupt" => GotStatus::CORRUPT,
        "FileLocked" => GotStatus::FILE_LOCKED,
        other => panic!("{other}"),
    }
}

#[test]
fn every_coded_triple_allows_exactly_the_listed_statuses_in_order() {
    assert_eq!(CODED.len(), 78);
    for (ft, ds, gs, want) in CODED {
        let got: Vec<&str> = status_check(file_type(ft), dat_status(ds), got_status(gs))
            .iter()
            .map(|s| s.name())
            .collect();
        assert_eq!(&got[..], *want, "{ft} {ds} {gs}");
        assert_eq!(
            baseline_status(file_type(ft), dat_status(ds), got_status(gs)).name(),
            want[0],
            "baseline of {ft} {ds} {gs}"
        );
    }
}

#[test]
fn every_other_triple_is_uncoded_and_yields_error() {
    let types = ["Dir", "Zip", "SevenZip", "File", "FileZip", "FileSevenZip"];
    let mut uncoded = 0;
    for ft in types {
        // Dat statuses 0..=5 (5 is the retired InDatMIA), got statuses 0..=3.
        for ds in 0..=5u8 {
            for gs in 0..=3u8 {
                let (dat, got) = (DatStatus(ds), GotStatus(gs));
                let coded = CODED
                    .iter()
                    .any(|(t, d, g, _)| *t == ft && dat_status(d) == dat && got_status(g) == got);
                let list = status_check(file_type(ft), dat, got);
                if coded {
                    assert!(!list.is_empty());
                } else {
                    uncoded += 1;
                    assert!(list.is_empty(), "{ft} {ds} {gs} should be uncoded");
                    assert_eq!(baseline_status(file_type(ft), dat, got), RepStatus::ERROR);
                }
            }
        }
    }
    // 6 types x 6 dat statuses x 4 got statuses = 144, 78 of them coded.
    assert_eq!(uncoded, 144 - 78);
}

#[test]
fn nodump_and_the_retired_mia_status_are_not_coded_and_other_types_are_too() {
    for gs in 0..=3u8 {
        assert!(status_check(FileType::FILE, DatStatus::IN_DAT_NO_DUMP, GotStatus(gs)).is_empty());
        assert!(
            status_check(FileType::FILE, DatStatus::RETIRED_IN_DAT_MIA, GotStatus(gs)).is_empty()
        );
        assert!(status_check(FileType::UNSET, DatStatus::IN_DAT_COLLECT, GotStatus(gs)).is_empty());
        assert!(status_check(FileType::FILE_ONLY, DatStatus::NOT_IN_DAT, GotStatus(gs)).is_empty());
    }
}

#[test]
fn names_match_the_enum_declaration_order() {
    let names = [
        "Error",
        "UnSet",
        "UnScanned",
        "DirCorrect",
        "DirMissing",
        "DirUnknown",
        "DirInToSort",
        "DirCorrupt",
        "Missing",
        "Correct",
        "NotCollected",
        "UnNeeded",
        "Unknown",
        "InToSort",
        "Corrupt",
        "Ignore",
        "CanBeFixed",
        "MoveToSort",
        "Delete",
        "NeededForFix",
        "Rename",
        "CorruptCanBeFixed",
        "MoveToCorrupt",
        "Deleted",
        "IncompleteRemove",
        "Incomplete",
        "MissingMIA",
        "CorrectMIA",
        "CanBeFixedMIA",
        "MissingNew",
    ];
    for (i, n) in names.iter().enumerate() {
        assert_eq!(RepStatus(i as u8).name(), *n);
    }
    assert_eq!(names.len(), RepStatus::END_VALUE);
    assert_eq!(RepStatus::default(), RepStatus::UN_SET);
}
