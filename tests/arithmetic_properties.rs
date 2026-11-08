#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Randomized laws supplement the fixed boundary/contract regression tests.
use kubectl_view_storage as storage;
use kubectl_view_storage::qty::{Bytes, ExactRatio};
use proptest::prelude::*;
mod support;

proptest! {
    #[test]
    fn ratios_agree_with_independent_u64_cross_products(a in any::<u64>(),b in 1..=u64::MAX,c in any::<u64>(),d in 1..=u64::MAX) {
        let left=ExactRatio::new(a as u128,b as u128).unwrap();
        let right=ExactRatio::new(c as u128,d as u128).unwrap();
        prop_assert_eq!(left.cmp(&right),((a as u128)*(d as u128)).cmp(&((c as u128)*(b as u128))));
    }
    #[test]
    fn ratio_order_is_transitive_with_u128_extremes(a in any::<u128>(),b in 1..=u128::MAX,c in any::<u128>(),d in 1..=u128::MAX,e in any::<u128>(),f in 1..=u128::MAX) {
        let x=ExactRatio::new(a,b).unwrap();let y=ExactRatio::new(c,d).unwrap();let z=ExactRatio::new(e,f).unwrap();
        prop_assert_eq!(x.partial_cmp(&y),Some(x.cmp(&y)));
        prop_assert_eq!(x==y,x.cmp(&y).is_eq());
        if x<=y && y<=z {prop_assert!(x<=z);}
    }
    #[test]
    fn integer_quantity_round_trip(value in any::<u64>()) {
        prop_assert_eq!(value.to_string().parse::<Bytes>().unwrap().get(),value);
    }
    #[test]
    fn parsers_never_panic_on_arbitrary_utf8(input in ".{0,300}") {
        let _=input.parse::<Bytes>();
    }
    #[test]
    fn adding_a_consumer_never_increases_root_capacity(capacities in prop::collection::vec(any::<u64>(),1..9), consumers in 0usize..12, claim in any::<usize>(), pv in any::<bool>()) {
        support::consumer_capacity_law(&capacities,consumers,claim,pv);
    }
    #[test]
    fn dimension_order_and_input_permutations_preserve_group_unions(capacities in prop::collection::vec(any::<u64>(),1..9), consumers in 0usize..12, keys in prop::collection::vec(any::<u8>(),7), prefix in 1usize..8, pv in any::<bool>()) {
        support::dimension_order_law(&capacities,consumers,&keys,prefix,pv);
    }
    #[test]
    fn unknown_quantities_remain_null(values in prop::collection::vec((any::<u64>(),any::<bool>()),1..9), pv in any::<bool>()) {
        support::unknown_quantity_law(&values,pv);
    }
    #[test]
    fn incomplete_membership_never_passes_either_strict_policy(capacities in prop::collection::vec(any::<u64>(),1..9), driver in any::<bool>(), pv in any::<bool>()) {
        support::incomplete_selection_law(&capacities,driver,pv);
    }
    #[test]
    fn unrelated_missing_or_invalid_field_cannot_suppress_pressure(scale in 1u64..=u64::MAX/100, percent in 80u8..=100, inodes in any::<bool>(), exhaustion in any::<bool>(), invalid in any::<bool>(), pv in any::<bool>()) {
        support::independent_pressure_law(scale,percent,inodes,exhaustion,invalid,pv);
    }
    #[test]
    fn record_and_group_quantity_order_is_permutation_invariant(values in prop::collection::vec((any::<u64>(),any::<bool>()),1..9), keys in prop::collection::vec(any::<u8>(),1..9), descending in any::<bool>(), pv in any::<bool>()) {
        support::requested_sort_law(&values,&keys,descending,pv);
    }
    #[test]
    fn record_and_group_ratio_order_matches_exact_cross_products(values in prop::collection::vec((any::<u64>(),any::<u64>(),any::<bool>()),1..9), keys in prop::collection::vec(any::<u8>(),1..9), descending in any::<bool>(), pv in any::<bool>()) {
        support::ratio_sort_law(&values,&keys,descending,pv);
    }
}
