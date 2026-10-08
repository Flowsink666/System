//! 根据固件内存描述管理真实物理页。只使用 ConventionalMemory，不回收固件或内核占用区。

const PAGE: usize = 4096;
const MAX_REGIONS: usize = 128;
const MAX_LIVE: usize = 256;

#[derive(Clone, Copy)]
struct Region {
    start: usize,
    next: usize,
    end: usize,
}

pub struct FrameAllocator {
    regions: [Region; MAX_REGIONS],
    count: usize,
    recycled: [usize; MAX_LIVE],
    recycled_count: usize,
    live: [usize; MAX_LIVE],
    live_count: usize,
    pub total_pages: usize,
}

impl Default for FrameAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameAllocator {
    pub const fn new() -> Self {
        Self {
            regions: [Region {
                start: 0,
                next: 0,
                end: 0,
            }; MAX_REGIONS],
            count: 0,
            recycled: [0; MAX_LIVE],
            recycled_count: 0,
            live: [0; MAX_LIVE],
            live_count: 0,
            total_pages: 0,
        }
    }

    pub fn add_region(&mut self, start: usize, pages: usize) -> bool {
        if pages == 0
            || start < 0x100000
            || !start.is_multiple_of(PAGE)
            || self.count == MAX_REGIONS
        {
            return false;
        }
        let Some(end) = pages
            .checked_mul(PAGE)
            .and_then(|size| start.checked_add(size))
        else {
            return false;
        };
        if self.regions[..self.count]
            .iter()
            .any(|region| start < region.end && end > region.start)
        {
            return false;
        }
        self.regions[self.count] = Region {
            start,
            next: start,
            end,
        };
        self.count += 1;
        self.total_pages += pages;
        true
    }

    pub fn allocate(&mut self) -> Option<usize> {
        if self.live_count == MAX_LIVE {
            return None;
        }
        let frame = if self.recycled_count != 0 {
            self.recycled_count -= 1;
            self.recycled[self.recycled_count]
        } else {
            let region = self.regions[..self.count]
                .iter_mut()
                .find(|region| region.next < region.end)?;
            let frame = region.next;
            region.next += PAGE;
            frame
        };
        self.live[self.live_count] = frame;
        self.live_count += 1;
        Some(frame)
    }

    pub fn free(&mut self, frame: usize) -> bool {
        let Some(index) = self.live[..self.live_count]
            .iter()
            .position(|value| *value == frame)
        else {
            return false;
        };
        self.live_count -= 1;
        self.live[index] = self.live[self.live_count];
        self.recycled[self.recycled_count] = frame;
        self.recycled_count += 1;
        true
    }

    pub fn allocated_pages(&self) -> usize {
        self.live_count
    }
    pub fn free_pages(&self) -> usize {
        self.total_pages - self.live_count
    }
    pub fn last_frame(&self) -> Option<usize> {
        self.live[..self.live_count].last().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_pages_are_unique_and_double_free_is_rejected() {
        let mut frames = FrameAllocator::new();
        assert!(frames.add_region(0x100000, 2));
        let first = frames.allocate().unwrap();
        let second = frames.allocate().unwrap();
        assert_ne!(first, second);
        assert!(frames.allocate().is_none());
        assert!(frames.free(first));
        assert!(!frames.free(first));
        assert!(!frames.free(0x123456));
        assert_eq!(frames.allocate(), Some(first));
        assert_eq!(frames.free_pages(), 0);
        assert!(!frames.add_region(0x100000, 1));
    }
    #[test]
    fn regions_reject_reserved_addresses_overlaps_and_overflow() {
        let mut frames = FrameAllocator::new();
        assert!(!frames.add_region(0, 10));
        assert!(!frames.add_region(0x100001, 1));
        assert!(!frames.add_region(0x100000, usize::MAX));
        assert!(frames.add_region(0x100000, 4));
        assert!(!frames.add_region(0x101000, 4));
        assert!(frames.add_region(0x200000, 2));
        assert_eq!(frames.total_pages, 6);
    }
}
