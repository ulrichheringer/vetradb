use vetra_io::FileIo;
use vetra_test_support::SimFile;

#[test]
fn io_contract_is_usable_by_an_independent_caller() {
    let mut file = SimFile::default();
    let provider: &mut dyn FileIo = &mut file;
    assert_eq!(provider.write_at(2, b"wal").unwrap(), 3);
    provider.sync_all().unwrap();
    file.power_loss(&[]).unwrap();
    let mut bytes = [0; 8];
    assert_eq!(file.read_at(0, &mut bytes).unwrap(), 5);
    assert_eq!(&bytes[..5], b"\0\0wal");
}
