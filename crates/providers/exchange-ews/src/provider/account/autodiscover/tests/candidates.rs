#[test]
fn candidates_try_domain_hosts_before_srv_hosts() {
    let candidates = domain_candidates("example.test");

    assert_eq!(
        candidates.iter().map(Url::as_str).collect::<Vec<_>>(),
        [
            "https://autodiscover.example.test/autodiscover/autodiscover.xml",
            "https://example.test/autodiscover/autodiscover.xml",
        ]
    );
}

#[test]
fn srv_candidates_sort_by_priority_then_weight_and_apply_the_limit() {
    let mut candidates = (0..MAX_SRV_RECORDS + 1)
        .map(|index| {
            let host = format!("host-{index}.example.test");
            SrvCandidate {
                priority: 20,
                weight: 0,
                url: candidate_url(&host, Some(443)).expect("synthetic SRV target parses"),
                host,
            }
        })
        .collect::<Vec<_>>();
    candidates.extend([
        SrvCandidate {
            priority: 10,
            weight: 1,
            url: candidate_url("low-weight.example.test", Some(443))
                .expect("synthetic SRV target parses"),
            host: "low-weight.example.test".to_owned(),
        },
        SrvCandidate {
            priority: 10,
            weight: 5,
            url: candidate_url("high-weight.example.test", Some(443))
                .expect("synthetic SRV target parses"),
            host: "high-weight.example.test".to_owned(),
        },
    ]);

    let ordered = order_srv_candidates(candidates);

    assert_eq!(ordered.len(), MAX_SRV_RECORDS);
    assert_eq!(ordered[0].host_str(), Some("high-weight.example.test"));
    assert_eq!(ordered[1].host_str(), Some("low-weight.example.test"));
    assert!(ordered[2..].iter().all(|candidate| {
        candidate
            .host_str()
            .is_some_and(|host| host.starts_with("host-"))
    }));
}
