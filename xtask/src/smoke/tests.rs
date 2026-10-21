use super::*;
use reqwest::header::HeaderValue;

fn options(args: &[&str]) -> Result<Options, &'static str> {
    Options::parse(&args.iter().map(|s| (*s).into()).collect::<Vec<_>>())
}

#[test]
fn smoke_requires_an_explicit_target_and_strict_arguments() {
    assert_eq!(options(&[]).err(), Some("missing_url"));
    for args in [vec!["--url"], vec!["--url", "http://localhost", "--unknown"], vec!["--url", "http://localhost", "--url", "http://localhost"], vec!["--url", "http://localhost", "--json", "--json"]] {
        assert!(options(&args).is_err());
    }
}

#[test]
fn smoke_accepts_only_an_origin_or_health_url_without_sensitive_components() {
    for url in ["https://example.invalid", "https://example.invalid/healthz", "http://127.0.0.1:8790/", "http://[::1]:8790/healthz"] {
        assert_eq!(options(&["--url", url]).unwrap().url.path(), "/healthz");
    }
    for url in ["relative", "file:///healthz", "ftp://example.invalid", "https://fixture-user@example.invalid", "https://example.invalid/?fixture", "https://example.invalid/#fixture", "https://example.invalid/api/fragments", " https://example.invalid", "https://example.invalid/\n"] {
        assert_eq!(options(&["--url", url]).err(), Some("invalid_url"));
    }
}

#[test]
fn smoke_timeout_and_expected_release_are_bounded() {
    assert_eq!(options(&["--url", "http://localhost"]).unwrap().timeout, Duration::from_secs(10));
    for ms in ["1", "60000"] {
        assert!(options(&["--url", "http://localhost", "--timeout-ms", ms]).is_ok());
    }
    for ms in ["0", "60001", "-1", "NaN", "18446744073709551616"] {
        assert_eq!(options(&["--url", "http://localhost", "--timeout-ms", ms]).err(), Some("invalid_timeout"));
    }
    for id in ["dev", "0123456789ab", "0123456789ab-dirty-123", "v1.0_test"] {
        assert!(options(&["--url", "http://localhost", "--expected-release", id]).is_ok());
    }
    for id in ["", "invalid release", "invalid\nrelease", &"a".repeat(65)] {
        assert_eq!(options(&["--url", "http://localhost", "--expected-release", id]).err(), Some("invalid_expected_release"));
    }
}

fn response(status: u16, values: &[&str], expected: Option<&str>) -> Report {
    let mut headers = HeaderMap::new();
    for value in values {
        headers.append("x-fragment-deploy", HeaderValue::from_str(value).unwrap());
    }
    Report::new(expected.map(str::to_owned)).response(status, &headers)
}

#[test]
fn smoke_needs_http_200_and_one_valid_release_even_without_an_expectation() {
    assert!(response(200, &["dev"], None).passed);
    assert!(response(200, &["0123456789ab"], Some("0123456789ab")).passed);
    for status in [201, 204, 301, 302, 307, 308, 401, 403, 404, 429, 500, 503] {
        let report = response(status, &["dev"], None);
        assert!(!report.passed);
        assert_eq!(report.code, "http_error");
        assert_eq!(report.http_status, Some(status));
    }
    assert_eq!(response(200, &[], None).code, "missing_release");
    for ids in [vec![""], vec!["fixture invalid"], vec!["dev", "dev"], vec!["dev", "other"], vec!["dev, other"]] {
        let report = response(200, &ids, None);
        assert_eq!(report.code, "invalid_release");
        assert!(report.release.is_none());
    }
    assert_eq!(response(200, &[&"a".repeat(65)], None).code, "invalid_release");
    let mut headers = HeaderMap::new();
    headers.insert("x-fragment-deploy", HeaderValue::from_bytes(&[0xff]).unwrap());
    assert_eq!(Report::new(None).response(200, &headers).code, "invalid_release");
}

#[test]
fn smoke_release_comparison_is_exact_and_safe_to_report() {
    for expected in ["012345", "0123456789AB", "0123456789ab-dirty-1"] {
        let report = response(200, &["0123456789ab"], Some(expected));
        assert!(!report.passed);
        assert_eq!(report.code, "release_mismatch");
        assert_eq!(report.release.as_deref(), Some("0123456789ab"));
    }
    let report = response(200, &["fixture invalid"], None);
    assert!(!report.text().contains("fixture invalid"));
    assert!(!serde_json::to_string(&report).unwrap().contains("fixture invalid"));
    assert!(report.text().contains("product journeys are untested"));
}

#[test]
fn smoke_json_contract_is_explicit_about_scope_and_unavailable_fields() {
    assert_eq!(serde_json::to_value(response(200, &["dev"], None)).unwrap(), serde_json::json!({
        "schema_version": 1, "scope": "health_and_deployment_identity_only", "passed": true,
        "code": "passed", "http_status": 200, "release": "dev", "expected_release": null
    }));
    let report = serde_json::to_value(Report::new(None)).unwrap();
    assert_eq!(report["passed"], false);
    assert!(report["http_status"].is_null());
    assert!(report["release"].is_null());
}
