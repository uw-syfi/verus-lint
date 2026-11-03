use vstd::prelude::*;
verus! {
pub fn max_of(v: &Vec<u32>) -> (r: u32) requires v.len() > 0 ensures forall|i: int| 0 <= i < v.len() ==> v[i] <= r {
    let mut m = v[0]; let mut i = 1;
    while i < v.len() invariant 1 <= i <= v.len(), forall|j: int| 0 <= j < i ==> v[j] <= m decreases v.len() - i {
        if v[i] > m { m = v[i]; } i += 1;
    }
    m
}
} // verus!
