//! Round-robin admission: a stream of invalidations cannot starve an image or transfer.
#[derive(Default)]
pub struct Schedule {
    next: usize,
}
impl Schedule {
    pub fn choose<T: Copy, const N: usize>(&mut self, ready: [(T, bool); N]) -> Option<T> {
        for offset in 0..N {
            let index = (self.next + offset) % N;
            if ready[index].1 {
                self.next = (index + 1) % N;
                return Some(ready[index].0);
            }
        }
        None
    }
}
