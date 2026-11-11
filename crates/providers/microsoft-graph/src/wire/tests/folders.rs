

#[tokio::test]
async fn folder_listing_follows_pages_and_traverses_children() {
    let server = MockServer::start_async().await;
    let page_two_link = format!(
        "{}/v1.0/me/mailFolders?$skiptoken=opaque-root-page",
        http_base_url(&server)
    );
    let root_page_one = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param_missing("$skiptoken");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "root-inbox", "displayName": "Inbox"}],
                    "@odata.nextLink": page_two_link,
                }));
        })
        .await;
    let root_page_two = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param("$skiptoken", "opaque-root-page");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "root-archive", "displayName": "Archive"}]
                }));
        })
        .await;
    let inbox_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-inbox/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "inbox-projects", "displayName": "Projects"}]
                }));
        })
        .await;
    let archive_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-archive/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"value": []}));
        })
        .await;
    let projects_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/inbox-projects/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"value": []}));
        })
        .await;
    let (api, transport) = synthetic_graph_api(&server);
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");

    let folders = api
        .list_folders(&account_id, synthetic_access_token())
        .await
        .expect("all synthetic root and child folders are listed");

    assert_eq!(
        folders
            .iter()
            .map(|folder| folder.reference().folder_id().as_str())
            .collect::<Vec<_>>(),
        ["root-inbox", "root-archive", "inbox-projects"]
    );
    assert_eq!(
        folders[2]
            .parent()
            .expect("synthetic Projects folder has its parent")
            .folder_id()
            .as_str(),
        "root-inbox"
    );
    root_page_one.assert_async().await;
    root_page_two.assert_async().await;
    inbox_children.assert_async().await;
    archive_children.assert_async().await;
    projects_children.assert_async().await;
    drop(transport);
}

#[tokio::test]
async fn folder_listing_rejects_a_repeated_page_link_before_refetching() {
    let server = MockServer::start_async().await;
    let (api, transport) = synthetic_graph_api(&server);
    let first_page = api
        .authority
        .root_folders_path()
        .expect("synthetic root folder URL is valid");
    let repeated_page = first_page.as_str().to_owned();
    let root_page = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param_missing("$skiptoken");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [],
                    "@odata.nextLink": repeated_page,
                }));
        })
        .await;
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");

    assert_eq!(
        api.list_folders(&account_id, synthetic_access_token())
            .await,
        Err(GraphError::UnsafeLink)
    );
    root_page.assert_calls_async(1).await;
    drop(transport);
}

#[tokio::test]
async fn folder_listing_rejects_duplicate_ids_in_the_folder_tree() {
    let server = MockServer::start_async().await;
    let roots = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders")
                .query_param_missing("$skiptoken");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [
                        {"id": "root-inbox", "displayName": "Inbox"},
                        {"id": "root-archive", "displayName": "Archive"}
                    ]
                }));
        })
        .await;
    let duplicate_child = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-inbox/childFolders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "value": [{"id": "root-archive", "displayName": "Duplicate"}]
                }));
        })
        .await;
    let unvisited_children = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1.0/me/mailFolders/root-archive/childFolders");
            then.status(200).json_body(json!({"value": []}));
        })
        .await;
    let (api, transport) = synthetic_graph_api(&server);
    let account_id =
        AccountId::try_from("synthetic-account".to_owned()).expect("synthetic account ID is valid");

    assert_eq!(
        api.list_folders(&account_id, synthetic_access_token())
            .await,
        Err(GraphError::InvalidRecord)
    );
    roots.assert_async().await;
    duplicate_child.assert_async().await;
    unvisited_children.assert_calls_async(0).await;
    drop(transport);
}

#[test]
fn missing_hidden_folder_flag_defaults_to_visible() {
    let folder: WireFolder = serde_json::from_value(json!({
        "id": "folder-1",
        "displayName": "Inbox"
    }))
    .expect("synthetic folder without isHidden is valid");

    assert!(!folder.is_hidden);
}

#[test]
fn maps_folder_payload_to_account_scoped_domain_values() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let folder = WireFolder {
        id: "folder-1".to_owned(),
        display_name: "Inbox".to_owned(),
        is_hidden: false,
    }
    .into_domain(&account_id, None)
    .expect("synthetic folder maps to the domain")
    .expect("synthetic folder is visible");
    assert_eq!(folder.reference().account_id(), &account_id);
}

#[test]
fn rejects_empty_folder_ids() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    assert!(
        WireFolder {
            id: String::new(),
            display_name: "Inbox".to_owned(),
            is_hidden: false,
        }
        .into_domain(&account_id, None)
        .is_err()
    );
}

#[test]
fn rejects_folder_metadata_that_lists_itself_as_parent() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let parent_id =
        FolderId::try_from("folder-1".to_owned()).expect("synthetic folder ID is non-empty");
    let folder = WireFolder {
        id: "folder-1".to_owned(),
        display_name: "Child".to_owned(),
        is_hidden: false,
    }
    .into_domain(&account_id, Some(parent_id));

    assert_eq!(folder.err(), Some(GraphError::InvalidRecord));
}

#[test]
fn excludes_hidden_folders_from_the_browsable_tree() {
    let account_id = AccountId::try_from("synthetic-account".to_owned())
        .expect("synthetic account ID is non-empty");
    let folder = WireFolder {
        id: "folder-hidden".to_owned(),
        display_name: "Hidden".to_owned(),
        is_hidden: true,
    }
    .into_domain(&account_id, None)
    .expect("hidden folder metadata is valid");

    assert!(folder.is_none());
}