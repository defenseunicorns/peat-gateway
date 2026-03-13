#![cfg(feature = "loadtest")]

use peat_gateway::loadtest;

#[tokio::test]
async fn load_test_mixed_produces_report() {
    let report = loadtest::run(2, 5, "mixed".into(), 3, None).await.unwrap();

    assert!(report.total_requests > 0, "expected >0 requests");
    assert!(report.throughput_rps > 0.0, "expected >0 throughput");
    assert_eq!(report.concurrency, 2);
    assert_eq!(report.scenario, "mixed");
    assert!(!report.endpoints.is_empty(), "expected endpoint stats");
}

#[tokio::test]
async fn load_test_burst_measures_throughput() {
    let report = loadtest::run(2, 5, "burst".into(), 3, None).await.unwrap();

    assert!(report.total_requests > 0);
    assert_eq!(report.scenario, "burst");

    // Burst is all formation creates — verify that endpoint shows up
    let has_creates = report
        .endpoints
        .iter()
        .any(|e| e.endpoint.contains("POST") && e.endpoint.contains("formations"));
    assert!(has_creates, "burst should have formation create requests");
}

#[tokio::test]
async fn load_test_json_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.json");
    let path_str = path.to_str().unwrap().to_string();

    let report = loadtest::run(2, 3, "mixed".into(), 3, Some(path_str))
        .await
        .unwrap();

    assert!(path.exists(), "JSON report file should exist");

    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(parsed["scenario"], "mixed");
    assert_eq!(parsed["concurrency"], report.concurrency);
    assert!(parsed["total_requests"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn load_test_multi_org_isolation() {
    let report = loadtest::run(4, 5, "multi-org".into(), 3, None)
        .await
        .unwrap();

    assert!(report.total_requests > 0);
    assert_eq!(report.scenario, "multi-org");
    assert_eq!(report.concurrency, 4);

    // Should have org-related endpoints (POST /orgs is called in setup for each org)
    let has_org_ops = report
        .endpoints
        .iter()
        .any(|e| e.endpoint.contains("/orgs"));
    assert!(has_org_ops, "multi-org should operate on org endpoints");
}

#[tokio::test]
async fn load_test_read_heavy_scenario() {
    let report = loadtest::run(2, 5, "read-heavy".into(), 3, None)
        .await
        .unwrap();

    assert!(report.total_requests > 0);
    assert_eq!(report.scenario, "read-heavy");

    // Read-heavy should have more read (GET/LIST) ops than writes
    let reads: usize = report
        .endpoints
        .iter()
        .filter(|e| e.endpoint.starts_with("GET"))
        .map(|e| e.count)
        .sum();
    let writes: usize = report
        .endpoints
        .iter()
        .filter(|e| e.endpoint.starts_with("POST") || e.endpoint.starts_with("DELETE"))
        .map(|e| e.count)
        .sum();
    assert!(
        reads > writes,
        "read-heavy should have more reads ({reads}) than writes ({writes})"
    );
}
