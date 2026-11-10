use httpmock::MockServer;

pub(super) fn http_url(server: &MockServer, path: &str) -> String {
    format!("http://{}{path}", server.address())
}

pub(super) fn http_base_url(server: &MockServer) -> String {
    http_url(server, "")
}
