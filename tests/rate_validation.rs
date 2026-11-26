use sinclicker::clicker::{click_interval_micros, validate_cps, DEFAULT_CPS, MAX_CPS, MIN_CPS};

#[test]
fn cps_range_is_enforced() {
    assert_eq!(MIN_CPS, 1);
    assert_eq!(MAX_CPS, 500);
    assert_eq!(DEFAULT_CPS, 10);

    assert_eq!(validate_cps(10), 10);
    assert_eq!(validate_cps(1), 1);
    assert_eq!(validate_cps(500), 500);
    assert_eq!(validate_cps(0), 1);
    assert_eq!(validate_cps(-1), 1);
    assert_eq!(validate_cps(501), 500);
    assert_eq!(validate_cps(i32::MAX), 500);
}

#[test]
fn interval_bounds() {
    assert_eq!(click_interval_micros(1), 1_000_000);
    assert_eq!(click_interval_micros(DEFAULT_CPS), 100_000);
    assert_eq!(click_interval_micros(500), 2_000);
    // The 500 CPS interval is the shortest supported one; nothing outside the
    // range may schedule clicks faster than 2 ms or slower than 1 s.
    assert_eq!(click_interval_micros(0), click_interval_micros(1));
    assert_eq!(click_interval_micros(10_000), click_interval_micros(500));
}
