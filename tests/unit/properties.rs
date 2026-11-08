//! Deterministic anchors exercise the same oracles as the Proptest suite.
use crate as storage;
#[path = "../support/mod.rs"]
mod support;

#[test]
fn accounting_selection_and_pressure_property_anchors() {
    for pv in [false, true] {
        for values in [vec![0], vec![1, 1, 0], vec![u64::MAX, 17, u64::MAX]] {
            for lifecycle in 0..4 {
                support::consumer_capacity_law(&values, 7, lifecycle, pv);
            }
            support::dimension_order_law(&values, 7, &[6, 2, 5, 0, 3, 1, 4], 7, pv);
            support::dimension_order_law(&values, 0, &[0, 1, 2, 3, 4, 5, 6], 3, pv);
            support::unknown_quantity_law(
                &values
                    .iter()
                    .enumerate()
                    .map(|(i, &value)| (value, i % 2 == 0))
                    .collect::<Vec<_>>(),
                pv,
            );
            support::incomplete_selection_law(&values, true, pv);
            support::incomplete_selection_law(&values, false, pv);
        }
        for inodes in [false, true] {
            for exhaustion in [false, true] {
                for invalid in [false, true] {
                    for percent in [80, 90, 100] {
                        support::independent_pressure_law(
                            u64::MAX,
                            percent,
                            inodes,
                            exhaustion,
                            invalid,
                            pv,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn record_and_group_order_property_anchors() {
    for pv in [false, true] {
        for descending in [false, true] {
            support::requested_sort_law(
                &[
                    (u64::MAX, false),
                    (0, false),
                    (0, true),
                    (42, false),
                    (42, false),
                    (u64::MAX, true),
                    (7, true),
                ],
                &[4, 3, 2, 1, 0],
                descending,
                pv,
            );
            support::ratio_sort_law(
                &[
                    (100, 50, false),
                    (200, 100, false),
                    (u64::MAX, u64::MAX - 1, false),
                    (1, 0, true),
                    (1, 0, false),
                ],
                &[4, 3, 2, 1, 0],
                descending,
                pv,
            );
        }
    }
}
