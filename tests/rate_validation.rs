use sinclicker::clicker::{click_interval, validate_cps, DEFAULT_CPS, MAX_CPS, MIN_CPS};

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
fn interval_is_inverse_of_cps() {
    for cps in 1..=500 {
        let interval = click_interval(cps);
        let seconds = interval.as_secs_f64();
        let expected = 1.0 / cps as f64;
        assert!(
            (seconds - expected).abs() < 1e-9,
            "cps {cps}: {seconds} != {expected}"
        );
    }
}

#[test]
fn interval_bounds() {
    assert_eq!(click_interval(1).as_millis(), 1000);
    assert_eq!(click_interval(500).as_millis(), 2);
    // The 500 CPS interval is the shortest supported one; nothing below the
    // range may schedule clicks faster than 2 ms.
    assert!(click_interval(0) > click_interval(500));
    assert!(click_interval(10_000) == click_interval(500));
}
