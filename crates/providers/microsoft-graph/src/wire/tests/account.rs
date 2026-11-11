

#[tokio::test]
async fn profile_read_does_not_follow_a_graph_redirect() {
    let server = MockServer::start_async().await;
    let redirect = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1.0/me");
            then.status(302).header("Location", "/redirected");
        })
        .await;
    let redirect_target = server
        .mock_async(|when, then| {
            when.method(GET).path("/redirected");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"synthetic-account"}"#);
        })
        .await;
    let (api, transport) = synthetic_graph_api(&server);
    let profile_endpoint = Url::parse(&http_url(&server, "/v1.0/me"))
        .expect("synthetic Graph profile endpoint is valid");

    assert_eq!(
        api.load_profile(&profile_endpoint, synthetic_access_token())
            .await,
        Err(GraphError::Transport)
    );
    redirect.assert_calls_async(1).await;
    redirect_target.assert_calls_async(0).await;
    drop(transport);
}