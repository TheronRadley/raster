use std::fs as stdfs;
use std::path::PathBuf;
use std::time::Duration;

use raster::app::buffer::Buffer;
use raster::config::NewlineMode;
use raster::core::document::Document;
use raster::core::transaction::Replacement;
use raster::fs as rfs;
use raster::fs::SaveOptions;
use raster::position::BytePos;
use raster::session::RecoveryManager;
use raster::util::{unique_suffix, XorShift64};

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("raster-property-{}", unique_suffix()));
        stdfs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = stdfs::remove_dir_all(&self.0);
    }
}

#[test]
fn randomized_transactions_match_a_string_reference_model() {
    let mut rng = XorShift64::new(0x51_7a_57_65);
    let fragments = [
        "a",
        "مرحبا",
        "e\u{301}",
        "👩‍💻",
        "漢",
        "\r\n",
        "\n",
        "\t",
        " ",
    ];
    let mut model = String::from("start");
    let mut buffer = Buffer::scratch(1, &model);
    for step in 0..150 {
        let boundaries: Vec<usize> = model
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(model.len()))
            .collect();
        let a = boundaries[rng.below(boundaries.len())];
        let b = boundaries[rng.below(boundaries.len())];
        let (start, end) = (a.min(b), a.max(b));
        let mut inserted = String::new();
        for _ in 0..rng.below(5) {
            inserted.push_str(fragments[rng.below(fragments.len())]);
        }
        let before = model.clone();
        let mut expected = model.clone();
        expected.replace_range(start..end, &inserted);
        let edits = vec![Replacement::new(
            BytePos(start)..BytePos(end),
            inserted.clone(),
        )];
        let after = raster::app::buffer::shifted_carets(&edits, &[BytePos(start + inserted.len())]);
        buffer
            .apply_edits("property", edits, after)
            .unwrap_or_else(|e| panic!("step {step}: {e}"));
        assert_eq!(buffer.document.text(), expected, "step {step}");
        assert!(buffer.undo().unwrap(), "step {step} undo");
        assert_eq!(buffer.document.text(), before, "step {step} undo content");
        assert!(buffer.redo().unwrap(), "step {step} redo");
        assert_eq!(buffer.document.text(), expected, "step {step} redo content");
        model = expected;
    }
}

#[test]
fn mixed_newlines_missing_final_newline_and_unicode_round_trip_exactly() {
    let dir = TestDir::new();
    let path = dir.0.join("roundtrip.txt");
    let original = "first\r\nsecond\nthird\rmixed مرحبا e\u{301} 👩‍💻";
    stdfs::write(&path, original.as_bytes()).unwrap();
    let (mut document, mut disk) = rfs::load_file(&path, false).unwrap();
    assert_eq!(document.text(), original);
    assert_eq!(
        document.to_bytes(NewlineMode::Preserve),
        original.as_bytes()
    );
    document = Document::from_text(&format!("{original}\nEDIT"));
    rfs::save_document(&document, &mut disk, SaveOptions::default()).unwrap();
    assert_eq!(
        stdfs::read(&path).unwrap(),
        format!("{original}\nEDIT").as_bytes()
    );
}

#[test]
fn external_change_blocks_save_and_dirty_reload_is_refused() {
    let dir = TestDir::new();
    let path = dir.0.join("external.txt");
    stdfs::write(&path, b"original").unwrap();
    let (document, disk) = rfs::load_file(&path, false).unwrap();
    let mut buffer = Buffer::from_document(1, "external.txt".into(), document, Some(path.clone()));
    let _disk = disk;
    buffer.set_carets(vec![BytePos(0)]);
    buffer.insert_text("!").unwrap();
    stdfs::write(&path, b"changed elsewhere").unwrap();
    assert!(matches!(
        buffer.check_external(),
        rfs::ExternalState::Modified
    ));
    assert!(matches!(
        buffer.save(SaveOptions::default()),
        Err(raster::app::buffer::BufferError::Save(
            rfs::SaveError::Conflict(_)
        ))
    ));
    assert_eq!(stdfs::read(&path).unwrap(), b"changed elsewhere");
    assert_eq!(
        buffer.reload(false, false),
        Err(raster::app::buffer::BufferError::DirtyReload)
    );
    assert_eq!(buffer.document.text(), "!original");
}

#[test]
fn multiple_dirty_buffers_have_private_independent_recovery_records() {
    let dir = TestDir::new();
    let recovery = RecoveryManager::start(dir.0.join("recovery")).unwrap();
    let a = Buffer::scratch(1, "alpha");
    let b = Buffer::scratch(2, "beta");
    let mut first = a.recovery_job();
    first.buffer_id = 1;
    let mut second = b.recovery_job();
    second.buffer_id = 2;
    recovery.submit(first);
    recovery.submit(second);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let records = loop {
        let records = recovery.recover_all();
        if records.len() >= 2 || std::time::Instant::now() > deadline {
            break records;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(records.len() >= 2, "recovery records: {}", records.len());
    assert!(records.iter().any(|r| r.document.text() == "alpha"));
    assert!(records.iter().any(|r| r.document.text() == "beta"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for record in &records {
            assert_eq!(
                stdfs::metadata(&record.file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
