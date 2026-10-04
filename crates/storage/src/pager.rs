//! Positioned checked page I/O. WAL/recovery must supply the publication barrier.
use crate::{
    Error, Result,
    codec::{Address, Image, PAGE_SIZE, Page, Superblock},
};
use vetra_io::{FileIo, read_exact_at, write_all_at};
use vetra_recovery_api::WalBarrier;
pub struct Pager<F: FileIo> {
    file: F,
    pub superblock: Superblock,
    selected: usize,
    poisoned: bool,
}
impl<F: FileIo> Pager<F> {
    pub fn bootstrap(mut file: F, superblock: Superblock) -> Result<Self> {
        if !file.is_empty()? {
            return Err(Error::Corrupt("bootstrap nonempty file"));
        }
        let b = superblock.encode()?;
        write_all_at(&mut file, 0, &b)?;
        write_all_at(&mut file, PAGE_SIZE as u64, &b)?;
        file.sync_all()?;
        Ok(Self {
            file,
            superblock,
            selected: 0,
            poisoned: false,
        })
    }
    pub fn open(file: F, retained_start: u64) -> Result<Self> {
        let len = file.len()?;
        if len < 2 * PAGE_SIZE as u64 || len % PAGE_SIZE as u64 != 0 {
            return Err(Error::Corrupt("data file length"));
        }
        let mut a = [0; PAGE_SIZE];
        let mut b = [0; PAGE_SIZE];
        read_exact_at(&file, 0, &mut a)?;
        read_exact_at(&file, PAGE_SIZE as u64, &mut b)?;
        let superblock = Superblock::select(&a, &b, retained_start)?;
        let selected = if Superblock::decode(&a).as_ref() == Ok(&superblock) {
            0
        } else {
            1
        };
        Ok(Self {
            file,
            superblock,
            selected,
            poisoned: false,
        })
    }
    pub fn read(&self, a: Address, owner: u64) -> Result<Page> {
        let offset = a.id.checked_mul(PAGE_SIZE as u64).ok_or(Error::Limit)?;
        let mut b = [0; PAGE_SIZE];
        read_exact_at(&self.file, offset, &mut b)?;
        Page::decode(&b, a, owner)
    }
    pub fn write(&mut self, page: &Page, wal: &mut impl WalBarrier) -> Result<()> {
        if self.poisoned {
            return Err(Error::Io(vetra_types::IoFailure::DurabilityFailure));
        }
        if page.lsn == 0 {
            return Err(Error::WalOrdering);
        }
        let b = page.encode()?;
        wal.durable_through(page.lsn)?;
        let offset = page
            .address
            .id
            .checked_mul(PAGE_SIZE as u64)
            .ok_or(Error::Limit)?;
        if let Err(e) = write_all_at(&mut self.file, offset, &b) {
            self.poisoned = true;
            return Err(e.into());
        }
        Ok(())
    }
    pub fn publish_superblock(
        &mut self,
        mut next: Superblock,
        wal: &mut impl WalBarrier,
    ) -> Result<()> {
        if self.poisoned {
            return Err(Error::Io(vetra_types::IoFailure::DurabilityFailure));
        }
        if next.database != self.superblock.database || next.timeline != self.superblock.timeline {
            return Err(Error::Corrupt("superblock lineage"));
        }
        next.generation = self
            .superblock
            .generation
            .checked_add(1)
            .ok_or(Error::Exhausted)?;
        let b = next.encode()?;
        wal.durable_through(next.checkpoint)?;
        let target = 1 - self.selected;
        let result = (|| -> std::result::Result<(), vetra_types::IoFailure> {
            self.file.sync_data()?;
            write_all_at(&mut self.file, (target * PAGE_SIZE) as u64, &b)?;
            self.file.sync_all()
        })();
        if let Err(e) = result {
            self.poisoned = true;
            return Err(e.into());
        }
        self.superblock = next;
        self.selected = target;
        Ok(())
    }
    pub fn into_file(self) -> F {
        self.file
    }
    pub fn raw_read(&self, a: Address) -> Result<Image> {
        let mut b = [0; PAGE_SIZE];
        read_exact_at(
            &self.file,
            a.id.checked_mul(PAGE_SIZE as u64).ok_or(Error::Limit)?,
            &mut b,
        )?;
        Ok(b)
    }
}

/// Checked bridge from the positioned pager to the bounded buffer. Both the cache
/// barrier and this provider barrier must succeed before any dirty write.
pub struct PagerIo<F: FileIo, W: WalBarrier> {
    pub pager: Pager<F>,
    pub wal: W,
}
impl<F: FileIo + Send, W: WalBarrier + Send> crate::buffer::PageIo for PagerIo<F, W> {
    fn load(&mut self, a: Address) -> Result<Image> {
        self.pager.raw_read(a)
    }
    fn write(&mut self, a: Address, image: &Image) -> Result<()> {
        let owner = u64::from_le_bytes(image[32..40].try_into().unwrap());
        let page = Page::decode(image, a, owner)?;
        self.pager.write(&page, &mut self.wal)
    }
}
