//! Round-robin admission: a stream of invalidations cannot starve an image or transfer.
#[derive(Default)]
pub struct Schedule {
    next: usize,
}
impl Schedule {
    pub fn choose<const N: usize>(&mut self, ready: [bool; N]) -> Option<usize> {
        for offset in 0..N {
            let index = (self.next + offset) % N;
            if ready[index] {
                self.next = (index + 1) % N;
                return Some(index);
            }
        }
        None
    }
}
