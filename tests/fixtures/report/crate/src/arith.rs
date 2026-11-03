use vstd::prelude::*;
verus! {
pub open spec fn tri(n: nat) -> nat decreases n { if n == 0 { 0 } else { n + tri((n - 1) as nat) } }
pub proof fn lemma_tri_mono(a: nat, b: nat) requires a <= b ensures tri(a) <= tri(b) decreases b {
    if a < b { lemma_tri_mono(a, (b - 1) as nat); }
}
pub fn sum_to(n: u64) -> (r: u64) requires n < 1000 ensures r == tri(n as nat) {
    let mut i: u64 = 0; let mut s: u64 = 0;
    while i < n invariant i <= n, n < 1000, s == tri(i as nat), s <= i * i decreases n - i {
        i = i + 1; s = s + i;
        assert(tri(i as nat) == i + tri((i - 1) as nat));
        assume(s <= i * i);
    }
    s
}
pub struct Pt { pub x: u8 }
impl Pt { pub fn bump(&mut self) requires old(self).x < 10 ensures final(self).x == old(self).x + 1 { self.x = self.x + 1; } }
} // verus!
