//! Fixture crate for verus-lint: every fact kind the extractor reads.
use vstd::prelude::*;

verus! {

pub mod q {
    use vstd::prelude::*;

    pub open spec fn p(i: int) -> bool { i >= 0 }
    pub open spec fn r(i: int) -> bool { i > 0 }

    // Explicit trigger, inline annotation.
    pub open spec fn inline_trig() -> bool { forall|i: int| #[trigger] p(i) ==> r(i) }
    // Explicit trigger, block form with two groups.
    pub open spec fn block_trig() -> bool {
        forall|i: int| #![trigger p(i)] #![trigger r(i)] p(i) ==> r(i)
    }
    // No trigger written: warning case.
    pub open spec fn no_trig() -> bool { forall|i: int| p(i) ==> r(i) }
    // Automatic triggers requested.
    pub open spec fn auto_trig() -> bool { forall|i: int| #![auto] p(i) ==> r(i) }
    // Existential without trigger.
    pub open spec fn exists_q() -> bool { exists|i: int| p(i) && r(i) }
}

pub mod t {
    use vstd::prelude::*;

    pub proof fn lemma_assume() { assume(1int == 2int); }
    pub proof fn lemma_admit() { admit(); }
    #[verifier::external_body]
    pub proof fn lemma_external() ensures 1int == 1int {}
    #[verifier::external_body]
    pub exec fn exec_external() -> (r: u64) ensures r == 7 { 7 }
    #[verifier::external_body]
    pub broadcast proof fn axiom_b(i: int) ensures #[trigger] crate::q::p(i) == (i >= 0) {}
    pub broadcast proof fn proven_b(i: int) ensures #[trigger] crate::q::r(i) == (i > 0) {}
    pub broadcast group group_b { axiom_b, proven_b }

    #[verifier::external]
    pub fn ext_id(x: u64) -> u64 { x }
    pub assume_specification[ ext_id ](x: u64) -> (r: u64) ensures r == x;
}

pub mod tr {
    use vstd::prelude::*;

    pub trait Shape {
        spec fn area(&self) -> int;
        open spec fn twice(&self) -> int { 2 * self.area() }
        spec fn required(&self) -> int;
    }
    pub struct Sq(pub int);
    pub struct Circ(pub int);
    impl Shape for Sq {
        open spec fn area(&self) -> int { self.0 * self.0 }
        open spec fn required(&self) -> int { 1 }
    }
    impl Shape for Circ {
        open spec fn area(&self) -> int { 3 * self.0 * self.0 }
        open spec fn twice(&self) -> int { 6 * self.0 * self.0 }
        open spec fn required(&self) -> int { 2 }
    }
    pub open spec fn use_shape<S: Shape>(s: S) -> int { s.twice() }
}

pub mod live {
    use vstd::prelude::*;
    use crate::q::*;

    pub proof fn lemma_used() { assert(p(1)); }
    pub proof fn lemma_dead_leaf() { assert(p(2)); }
    pub proof fn lemma_dead_a() { lemma_dead_b(); }
    pub proof fn lemma_dead_b() { lemma_dead_a(); }
    pub proof fn theorem_top() {
        lemma_used();
        let s = crate::tr::Sq(2);
        assert(crate::tr::use_shape(s) == crate::tr::use_shape(s));
    }
    pub broadcast proof fn lemma_in_group(i: int) ensures #[trigger] crate::q::p(i) == (i >= 0) { }
    pub broadcast group group_live { lemma_in_group }
    pub open spec fn dead_spec() -> bool { p(3) }
    proof fn lemma_via_broadcast() { broadcast use group_live; }

    pub exec fn run() { proof { lemma_via_broadcast(); } }
}

} // verus!

#[cfg(test)]
mod tests {
    #[test]
    fn it_runs() { assert_eq!(1, 1); }
}
