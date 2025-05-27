use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::{collections::btree_map::BTreeMap, sync::Arc};
use axalloc::GlobalPage;
use axerrno::{AxError, AxResult};
use axhal::mem::virt_to_phys;
use kspin::{SpinNoIrq, SpinRaw};
use lazyinit::LazyInit;
use memory_addr::PhysAddr;

static PAGE_MANAGER: LazyInit<SpinNoIrq<PageManager>> = LazyInit::new();

pub(crate) fn init_page_manager() {
    PAGE_MANAGER.init_once(SpinNoIrq::new(PageManager::new()));
}

pub fn page_manager() -> &'static SpinNoIrq<PageManager> {
    &PAGE_MANAGER
}

pub struct PageManager {
    phys2page: BTreeMap<PhysAddr, Arc<Page>>,
}

impl PageManager {
    pub fn new() -> Self {
        Self {
            phys2page: BTreeMap::new(),
        }
    }

    pub fn alloc(&mut self, zeroed: bool) -> AxResult<PhysAddr> {
        let res = GlobalPage::alloc();
        match res {
            Ok(mut page) => {
                if zeroed {
                    page.zero();
                }
                let paddr = page.start_paddr(virt_to_phys);
                debug!(
                    "page manager => alloc : {:#?}, zeroed : {}, size = {}",
                    paddr,
                    zeroed,
                    self.phys2page.len()
                );

                self.phys2page.insert(paddr, Arc::new(Page::new(page)));
                Ok(paddr)
            }
            Err(e) => Err(e),
        }
    }

    pub fn dealloc(&mut self, paddr: PhysAddr) {
        debug!("page manager => dealloc : {:#?}", paddr);
        self.sub_page_ref(paddr);
    }

    pub fn add_page_ref(&self, paddr: PhysAddr) {
        if let Some(page) = self.find_page(paddr) {
            debug!("page manager => add ref : {:#?}", paddr);
            page.add_ref_count();
        }
    }

    pub fn sub_page_ref(&mut self, paddr: PhysAddr) {
        if let Some(page) = self.find_page(paddr) {
            match page.sub_ref_count() {
                1 => {
                    debug!("page manager => sub ref : {:#?}. ref : 0", paddr);
                    self.phys2page.remove(&paddr);
                }
                n => debug!("page manager => sub ref : {:#?}, ref : {}", paddr, n - 1),
            }
        }
    }

    pub fn page_ref(&self, paddr: PhysAddr) -> usize {
        if let Some(page) = self.find_page(paddr) {
            page.ref_count()
        } else {
            0
        }
    }

    pub fn copy_page(&mut self, old_addr: PhysAddr, new_addr: PhysAddr) -> AxResult<()> {
        debug!(
            "page manager => copy page, old: {:#?}, new: {:#?}",
            old_addr, new_addr
        );
        let new_page = {
            if let Some(new_page) = self.find_page(new_addr) {
                new_page.clone()
            } else {
                // TODO: err
                return Err(AxError::NoMemory);
            }
        };

        let old_page = {
            if let Some(old_page) = self.find_page(old_addr) {
                old_page.clone()
            } else {
                debug!("not found old_page");
                // TODO: err
                return Err(AxError::NoMemory);
            }
        };

        let mut new_page = new_page.inner.lock();
        let old_page = old_page.inner.lock();

        new_page.as_slice_mut().copy_from_slice(old_page.as_slice());
        Ok(())
    }

    fn find_page(&self, addr: PhysAddr) -> Option<Arc<Page>> {
        if let Some((_, value)) = self.phys2page.range(..=addr).next_back() {
            if value.contain_paddr(addr) {
                Some(value.clone())
            } else {
                None
            }
        } else {
            None
        }
    }
}

pub struct Page {
    inner: SpinRaw<GlobalPage>,
    ref_count: AtomicUsize,
}

impl Page {
    fn new(page: GlobalPage) -> Self {
        Self {
            inner: SpinRaw::new(page),
            ref_count: AtomicUsize::new(1),
        }
    }

    pub fn add_ref_count(&self) -> usize {
        self.ref_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn sub_ref_count(&self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::SeqCst)
    }

    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::SeqCst)
    }

    pub fn contain_paddr(&self, addr: PhysAddr) -> bool {
        let page = self.inner.lock();
        let start = page.start_paddr(virt_to_phys);

        start <= addr && addr <= start + page.size()
    }
}
