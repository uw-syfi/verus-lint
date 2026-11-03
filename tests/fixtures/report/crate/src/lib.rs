use vstd::prelude::*;
pub mod arith;
pub mod seqs;
verus! {
pub fn top(x: u32) -> (r: u32)
    requires x < 100,
    ensures r == x + 1,
{
    x + 1
}
} // verus!
