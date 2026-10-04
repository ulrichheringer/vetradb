#![allow(dead_code)]
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use vetra_engine::{Database, Limits, Lineage};
use vetra_io::{DirectoryIo, FileIo};
use vetra_types::IoFailure;
use vetra_wal::Wal;
pub const LINEAGE: Lineage = Lineage {
    database: [1; 16],
    timeline: [2; 16],
};
#[derive(Default, Clone)]
struct Bytes {
    visible: Vec<u8>,
    synced: Vec<u8>,
}
#[derive(Default, Clone)]
struct State {
    files: BTreeMap<String, Bytes>,
    namespace: BTreeMap<String, ()>,
    events: usize,
    trace: Vec<String>,
    fail_at: Option<usize>,
    short: Option<usize>,
}
#[derive(Default, Clone)]
pub struct Directory {
    state: Arc<Mutex<State>>,
}
#[derive(Clone)]
pub struct File {
    state: Arc<Mutex<State>>,
    name: String,
}
fn event(state: &mut State, kind: &str) -> Result<(), IoFailure> {
    let at = state.events;
    state.events += 1;
    state.trace.push(kind.to_owned());
    if state.fail_at == Some(at) {
        return Err(IoFailure::DurabilityFailure);
    }
    Ok(())
}
impl Directory {
    pub fn fork(&self) -> Self {
        Self {
            state: Arc::new(Mutex::new(self.state.lock().unwrap().clone())),
        }
    }
    /// Unknown response may leave every completed OS write persistent. This is an explicit survivor plan.
    pub fn persist_visible(&self) {
        let mut s = self.state.lock().unwrap();
        for f in s.files.values_mut() {
            f.synced.clone_from(&f.visible);
        }
        s.fail_at = None;
    }
    pub fn crash(&self) {
        let mut s = self.state.lock().unwrap();
        let names = s.namespace.clone();
        s.files.retain(|n, _| names.contains_key(n));
        for file in s.files.values_mut() {
            file.visible.clone_from(&file.synced);
        }
        s.fail_at = None;
    }
    pub fn fail_after(&self, n: usize) {
        let mut s = self.state.lock().unwrap();
        s.fail_at = Some(s.events + n);
    }
    pub fn clear_failure(&self) {
        self.state.lock().unwrap().fail_at = None;
    }
    pub fn events(&self) -> usize {
        self.state.lock().unwrap().events
    }
    pub fn trace(&self) -> Vec<String> {
        self.state.lock().unwrap().trace.clone()
    }
    pub fn short_writes(&self, n: usize) {
        self.state.lock().unwrap().short = Some(n);
    }
    pub fn durable(&self, name: &str) -> Vec<u8> {
        self.state.lock().unwrap().files[name].synced.clone()
    }
    pub fn overwrite(&self, name: &str, bytes: Vec<u8>) {
        let mut s = self.state.lock().unwrap();
        s.files.get_mut(name).unwrap().visible = bytes.clone();
        s.files.get_mut(name).unwrap().synced = bytes;
    }
    pub fn append_unsynced(&self, name: &str, bytes: &[u8]) {
        self.state
            .lock()
            .unwrap()
            .files
            .get_mut(name)
            .unwrap()
            .visible
            .extend(bytes);
    }
    pub fn open_database(&self) -> Database {
        self.try_database().unwrap()
    }
    pub fn try_database(&self) -> Result<Database, vetra_engine::Error> {
        Database::from_log(
            Box::new(Wal::open(self.clone(), LINEAGE)?),
            Limits::default(),
        )
    }
}
impl DirectoryIo for Directory {
    type File = File;
    type Ownership = ();
    fn acquire_exclusive(&mut self) -> Result<(), IoFailure> {
        Ok(())
    }
    fn open(&mut self, name: &str) -> Result<File, IoFailure> {
        let mut s = self.state.lock().unwrap();
        if !s.files.contains_key(name) {
            event(&mut s, "create")?;
            s.files.insert(name.to_owned(), Bytes::default());
        }
        Ok(File {
            state: self.state.clone(),
            name: name.to_owned(),
        })
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), IoFailure> {
        let mut s = self.state.lock().unwrap();
        event(&mut s, "rename")?;
        let file = s.files.remove(from).ok_or(IoFailure::InvalidInput)?;
        s.files.insert(to.to_owned(), file);
        Ok(())
    }
    fn files(&self) -> Result<Vec<String>, IoFailure> {
        Ok(self.state.lock().unwrap().files.keys().cloned().collect())
    }
    fn sync_directory(&mut self) -> Result<(), IoFailure> {
        let mut s = self.state.lock().unwrap();
        event(&mut s, "directory sync")?;
        s.namespace = s.files.keys().cloned().map(|n| (n, ())).collect();
        Ok(())
    }
}
impl FileIo for File {
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<usize, IoFailure> {
        let s = self.state.lock().unwrap();
        let b = &s.files[&self.name].visible;
        let o = offset as usize;
        if o >= b.len() {
            return Ok(0);
        }
        let n = out.len().min(b.len() - o);
        out[..n].copy_from_slice(&b[o..o + n]);
        Ok(n)
    }
    fn write_at(&mut self, offset: u64, input: &[u8]) -> Result<usize, IoFailure> {
        let mut s = self.state.lock().unwrap();
        event(&mut s, "write")?;
        let n = s.short.unwrap_or(input.len()).min(input.len());
        if n == 0 {
            return Ok(0);
        }
        let b = &mut s.files.get_mut(&self.name).unwrap().visible;
        let o = offset as usize;
        b.resize(b.len().max(o + n), 0);
        b[o..o + n].copy_from_slice(&input[..n]);
        Ok(n)
    }
    fn len(&self) -> Result<u64, IoFailure> {
        Ok(self.state.lock().unwrap().files[&self.name].visible.len() as u64)
    }
    fn truncate(&mut self, len: u64) -> Result<(), IoFailure> {
        let mut s = self.state.lock().unwrap();
        event(&mut s, "truncate")?;
        s.files
            .get_mut(&self.name)
            .unwrap()
            .visible
            .resize(len as usize, 0);
        Ok(())
    }
    fn sync_data(&mut self) -> Result<(), IoFailure> {
        let mut s = self.state.lock().unwrap();
        event(&mut s, "file sync")?;
        let f = s.files.get_mut(&self.name).unwrap();
        f.synced.clone_from(&f.visible);
        Ok(())
    }
    fn sync_all(&mut self) -> Result<(), IoFailure> {
        self.sync_data()
    }
}
