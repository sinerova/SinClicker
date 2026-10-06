use sinclicker::clicker::{click_interval, validate_cps, DEFAULT_CPS, MAX_CPS, MIN_CPS};

#[test]
fn cps_range_is_enforced() {
    assert_eq!(MIN_CPS, 1);
    assert_eq!(MAX_CPS, 100);
    assert_eq!(DEFAULT_CPS, 10);

    assert_eq!(validate_cps(10), 10);
    assert_eq!(validate_cps(1), 1);
    assert_eq!(validate_cps(100), 100);
    assert_eq!(validate_cps(0), 1);
    assert_eq!(validate_cps(-1), 1);
    assert_eq!(validate_cps(101), 100);
    assert_eq!(validate_cps(i32::MAX), 100);
}

#[test]
fn interval_is_inverse_of_cps() {
    for cps in 1..=100 {
        let interval = click_interval(cps);
        let seconds = interval.as_secs_f64();
        let expected = 1.0 / cps as f64;
        assert!(
            (seconds - expected).abs() < 1e-6,
            "cps {cps}: {seconds} != {expected}"
        );
    }
}

#[test]
fn interval_bounds() {
    assert_eq!(click_interval(1).as_millis(), 1000);
    assert_eq!(click_interval(100).as_millis(), 10);
}
